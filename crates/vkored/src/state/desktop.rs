//! Transactional desktop preferences scoped to a vKore state directory.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, TransactionBehavior, limits::Limit, params};
use rustix::fs::{Mode, OFlags};

pub const DATABASE_NAME: &str = "state.sqlite3";
pub const DEFAULT_PORT: u16 = 8080;
const APPLICATION_ID: i64 = 0x564B4F52;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err("appearance must be system, light or dark".into()),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    ExtraLarge,
}

impl TextSize {
    pub const ALL: [Self; 4] = [Self::Small, Self::Default, Self::Large, Self::ExtraLarge];

    fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Default => "default",
            Self::Large => "large",
            Self::ExtraLarge => "extra-large",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontPreset {
    #[default]
    Grotesk,
    Sans,
    Mono,
}

impl FontPreset {
    pub const ALL: [Self; 3] = [Self::Grotesk, Self::Sans, Self::Mono];

    fn key(self) -> &'static str {
        match self {
            Self::Grotesk => "grotesk",
            Self::Sans => "sans",
            Self::Mono => "mono",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub appearance: Appearance,
    pub text_size: TextSize,
    pub font_preset: FontPreset,
    pub api_port: u16,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            text_size: TextSize::default(),
            font_preset: FontPreset::default(),
            api_port: DEFAULT_PORT,
        }
    }
}

pub enum Preference {
    Appearance(Appearance),
    TextSize(TextSize),
    Font(FontPreset),
    ResetTypography,
    ApiPort(u16),
}

pub struct Store(Connection);

impl Store {
    /// Open the database and import existing settings once, in the schema
    /// creation transaction. Leave the old files available to older builds.
    pub fn open(root: &Path) -> Result<(Self, Preferences, Vec<String>), String> {
        // SQLite's NOFOLLOW also checks parent components. Resolve directory
        // aliases such as macOS /var before opening the database leaf.
        let root = fs::canonicalize(root).map_err(describe)?;
        let path = root.join(DATABASE_NAME);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&path).map_err(describe)?.is_file() {
                    return Err(format!("{DATABASE_NAME} must be a regular file"));
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(describe)?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(describe)?;
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(describe)?;
        connection
            .set_limit(Limit::SQLITE_LIMIT_LENGTH, 4096)
            .map_err(describe)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(describe)?;
        let application_id: i64 = transaction
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(describe)?;
        let version: i64 = transaction
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(describe)?;
        let mut warnings = Vec::new();
        let preferences = match (application_id, version) {
            (0, 0) => {
                let has_schema: bool = transaction
                    .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                        row.get(0)
                    })
                    .map_err(describe)?;
                if has_schema {
                    return Err(format!("{DATABASE_NAME} belongs to another application"));
                }
                let preferences = migrate_legacy(&root, &mut warnings)?;
                transaction.execute_batch(
                    "CREATE TABLE desktop_preferences (
                        id INTEGER PRIMARY KEY CHECK (id = 1),
                        appearance TEXT NOT NULL CHECK (appearance IN ('system', 'light', 'dark')),
                        text_size TEXT NOT NULL CHECK (text_size IN ('small', 'default', 'large', 'extra-large')),
                        font_preset TEXT NOT NULL CHECK (font_preset IN ('grotesk', 'sans', 'mono')),
                        api_port INTEGER NOT NULL CHECK (api_port BETWEEN 1 AND 65535)
                    ) STRICT;",
                ).map_err(describe)?;
                transaction
                    .execute(
                        "INSERT INTO desktop_preferences VALUES (1, ?1, ?2, ?3, ?4)",
                        params![
                            preferences.appearance.key(),
                            preferences.text_size.key(),
                            preferences.font_preset.key(),
                            preferences.api_port
                        ],
                    )
                    .map_err(describe)?;
                transaction
                    .pragma_update(None, "application_id", APPLICATION_ID)
                    .map_err(describe)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(describe)?;
                preferences
            }
            (APPLICATION_ID, SCHEMA_VERSION) => read_preferences(&transaction)?,
            (APPLICATION_ID, _) => {
                return Err(format!("Unsupported state database version {version}"));
            }
            _ => return Err(format!("{DATABASE_NAME} belongs to another application")),
        };
        transaction.commit().map_err(describe)?;
        Ok((Self(connection), preferences, warnings))
    }

    /// Save only the field that changed, so different windows can change
    /// independent settings without overwriting one another.
    pub fn save(&self, preference: Preference) -> Result<(), String> {
        let rows = match preference {
            Preference::Appearance(value) => self.0.execute(
                "UPDATE desktop_preferences SET appearance = ?1 WHERE id = 1",
                [value.key()],
            ),
            Preference::TextSize(value) => self.0.execute(
                "UPDATE desktop_preferences SET text_size = ?1 WHERE id = 1",
                [value.key()],
            ),
            Preference::Font(value) => self.0.execute(
                "UPDATE desktop_preferences SET font_preset = ?1 WHERE id = 1",
                [value.key()],
            ),
            Preference::ResetTypography => self.0.execute(
                "UPDATE desktop_preferences SET text_size = ?1, font_preset = ?2 WHERE id = 1",
                [TextSize::default().key(), FontPreset::default().key()],
            ),
            Preference::ApiPort(value) => self.0.execute(
                "UPDATE desktop_preferences SET api_port = ?1 WHERE id = 1",
                [value],
            ),
        }
        .map_err(describe)?;
        if rows != 1 {
            return Err("Desktop preferences record is missing".into());
        }
        Ok(())
    }
}

fn read_preferences(connection: &Connection) -> Result<Preferences, String> {
    let (appearance, size, font, api_port): (String, String, String, u16) = connection
        .query_row(
            "SELECT appearance, text_size, font_preset, api_port FROM desktop_preferences WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).map_err(describe)?;
    let (text_size, font_preset) =
        parse_typography(&size, &font).ok_or("Invalid text preferences in state database")?;
    if api_port == 0 {
        return Err("Invalid API port in state database".into());
    }
    Ok(Preferences {
        appearance: Appearance::parse(&appearance)?,
        text_size,
        font_preset,
        api_port,
    })
}

fn parse_typography(size: &str, font: &str) -> Option<(TextSize, FontPreset)> {
    Some((
        TextSize::ALL
            .into_iter()
            .find(|value| value.key() == size)?,
        FontPreset::ALL
            .into_iter()
            .find(|value| value.key() == font)?,
    ))
}

pub fn parse_port(value: &str) -> Result<u16, String> {
    let text = value.trim();
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("enter a port from 1 to 65535".into());
    }
    text.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "enter a port from 1 to 65535".into())
}

fn migrate_legacy(root: &Path, warnings: &mut Vec<String>) -> Result<Preferences, String> {
    let appearance = legacy(root, "appearance", 16, warnings, |value| {
        Appearance::parse(value).ok()
    })?;
    let typography = legacy(root, "typography", 64, warnings, |value| {
        let mut keys = value.split_whitespace();
        let pair = parse_typography(keys.next()?, keys.next()?)?;
        keys.next().is_none().then_some(pair)
    })?;
    let api_port = legacy(root, "api-port", 16, warnings, |value| {
        parse_port(value).ok()
    })?;
    let (text_size, font_preset) = typography.unwrap_or_default();
    Ok(Preferences {
        appearance: appearance.unwrap_or_default(),
        text_size,
        font_preset,
        api_port: api_port.unwrap_or(DEFAULT_PORT),
    })
}

fn legacy<T>(
    root: &Path,
    name: &str,
    limit: usize,
    warnings: &mut Vec<String>,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    let path = root.join(name);
    let invalid = |warnings: &mut Vec<String>| {
        warnings.push(format!(
            "Invalid legacy {name} preference. Using the default."
        ));
        Ok(None)
    };
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => return invalid(warnings),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not import {name}: {error}")),
    }
    let file = rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(fs::File::from)
    .map_err(|error| format!("Could not import {name}: {error}"))?;
    if !file.metadata().map_err(describe)?.is_file() {
        return invalid(warnings);
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not import {name}: {error}"))?;
    if bytes.len() <= limit
        && let Some(value) = std::str::from_utf8(&bytes).ok().and_then(parse)
    {
        return Ok(Some(value));
    }
    invalid(warnings)
}

fn describe(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_legacy_preferences_once_and_saves_independent_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        fs::write(path.join("appearance"), "light\n").unwrap();
        fs::write(path.join("typography"), "large sans\n").unwrap();
        fs::write(path.join("api-port"), "9090\n").unwrap();
        let (first, initial, warnings) = Store::open(path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            initial,
            Preferences {
                appearance: Appearance::Light,
                text_size: TextSize::Large,
                font_preset: FontPreset::Sans,
                api_port: 9090,
            },
        );

        fs::write(path.join("appearance"), "system\n").unwrap();
        let (second, reopened, _) = Store::open(path).unwrap();
        assert_eq!(reopened, initial);
        first
            .save(Preference::Appearance(Appearance::Dark))
            .unwrap();
        second.save(Preference::Font(FontPreset::Mono)).unwrap();
        assert!(second.save(Preference::ApiPort(0)).is_err());
        drop((first, second));

        let (_, reopened, _) = Store::open(path).unwrap();
        assert_eq!(
            reopened,
            Preferences {
                appearance: Appearance::Dark,
                font_preset: FontPreset::Mono,
                ..initial
            },
        );
    }

    #[test]
    fn defaults_only_invalid_legacy_values() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        fs::write(path.join("appearance"), "dark\n").unwrap();
        fs::write(path.join("typography"), b"large host-font extra").unwrap();
        fs::write(path.join("api-port"), "8".repeat(100)).unwrap();
        let (_, preferences, warnings) = Store::open(path).unwrap();
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            preferences,
            Preferences {
                appearance: Appearance::Dark,
                ..Preferences::default()
            },
        );
    }

    #[test]
    fn refuses_to_initialize_an_unrelated_database() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(DATABASE_NAME);
        let foreign = Connection::open(&path).unwrap();
        foreign
            .execute("CREATE TABLE records (name TEXT)", [])
            .unwrap();
        drop(foreign);
        let before = fs::read(&path).unwrap();
        let Err(error) = Store::open(root.path()) else {
            panic!("opened a foreign database");
        };
        assert!(error.contains("belongs to another application"));
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
