//! Hardware form. Admission and defaults live in vkored; this module only
//! edits a draft and presents the current host snapshot.
use crate::typography::Typography;
use crate::{
    Message, UiElement, kit,
    machines::MachineMessage,
    theme,
    visuals::{self, Icon},
    widgets,
};
use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, slider, space, text_input, toggler};
use vkore_vm::hardware::{GuestNuma, HostResources, NetworkMode, PageSize, Resources};
use vkored::{MachineConfig, hardware::Requirements};

#[derive(Clone, Debug)]
pub struct Draft {
    pub cpus: u16,
    pub memory_mib: u32,
    memory_gib: String,
    pub disk_mib: u64,
    disk_gib: String,
    pub resources: Resources,
    accel: vkore_vm::AccelChoice,
    vendor: Option<vkore_vm::Vendor>,
    pub minimum: Requirements,
    pub storage_fixed: bool,
    pub show_cores: bool,
    pub show_storage: bool,
}
#[derive(Clone, Debug)]
pub enum Change {
    Passthrough(vkore_vm::passthrough::Selection),
    Cpus(u16),
    GuestNuma(GuestNuma),
    Network(NetworkMode),
    Gpu(vkore_vm::passthrough::gpu::Assignment, bool),
    Memory(u32),
    MemoryText(String),
    Disk(u32),
    DiskText(String),
    PinCpus,
    CpuPowerManagement,
    PinMemory,
    Reserve,
    Page(PageSize),
    Core(u32),
    Placement(Option<&'static str>),
    ShowCores,
    ShowStorage,
    Defaults,
}
impl Draft {
    pub fn new(host: Option<&HostResources>, minimum: Requirements) -> Self {
        let (cpus, memory_mib, disk_mib, resources) = host
            .map(|h| vkored::hardware::defaults(h, minimum))
            .unwrap_or((
                minimum.cpus,
                minimum.memory_mib,
                minimum.disk_mib.max(vkored::hardware::DEFAULT_DISK_MIB),
                Resources::default(),
            ));
        Self {
            cpus,
            memory_mib,
            memory_gib: size_text(u64::from(memory_mib)),
            disk_mib,
            disk_gib: size_text(disk_mib),
            resources,
            accel: vkore_vm::AccelChoice::Auto,
            vendor: None,
            minimum,
            storage_fixed: false,
            show_cores: false,
            show_storage: false,
        }
    }
    pub fn from_config(c: &MachineConfig, storage_fixed: bool) -> Self {
        Self {
            cpus: c.cpus,
            memory_mib: c.memory_mib,
            memory_gib: size_text(u64::from(c.memory_mib)),
            disk_mib: c.disk_mib,
            disk_gib: size_text(c.disk_mib),
            resources: c.resources.clone(),
            accel: c.accel,
            vendor: c.vendor,
            minimum: c.requirements,
            storage_fixed,
            show_cores: false,
            show_storage: false,
        }
    }
    pub fn apply(&self, config: &mut MachineConfig) {
        config.cpus = self.cpus;
        config.memory_mib = self.memory_mib;
        config.disk_mib = self.disk_mib;
        config.resources = self.resources.clone();
        config.requirements = self.minimum;
    }
    fn recommended_cpu_waits(&self, host: &HostResources) -> bool {
        vkored::hardware::recommended_cpu_waits(
            host,
            self.resources.pin_cpus,
            self.accel,
            self.vendor,
        )
    }
    pub fn change(&mut self, change: Change, host: &HostResources) {
        match change {
            Change::Passthrough(selection) => {
                if let Some(index) = self
                    .resources
                    .passthrough
                    .iter()
                    .position(|s| *s == selection)
                {
                    self.resources.passthrough.remove(index);
                } else if self.resources.passthrough.len() + self.resources.gpu_vfs.len()
                    < vkore_vm::passthrough::MAX_DEVICES
                    && host.npus.devices.iter().any(|device| {
                        vkore_vm::passthrough::Selection::from_device(device).as_ref()
                            == Some(&selection)
                            && vkore_vm::passthrough::eligible(device).is_ok()
                    })
                {
                    self.resources.passthrough.push(selection);
                }
            }
            Change::Gpu(selection, enabled) => {
                if !enabled {
                    self.resources
                        .gpu_vfs
                        .retain(|s| s.physical_function != selection.physical_function);
                } else if selection.parent(&host.gpus).is_ok() {
                    let mut next = self.resources.gpu_vfs.clone();
                    next.retain(|s| s.physical_function != selection.physical_function);
                    next.push(selection);
                    if vkore_vm::passthrough::gpu::validate(&next, self.resources.passthrough.len())
                        .is_ok()
                    {
                        self.resources.gpu_vfs = next;
                    }
                }
            }
            Change::Cpus(n) => {
                self.cpus = n;
                let preferred = host.preferred_cpus(u16::MAX);
                self.resources.host_cpus.retain(|id| preferred.contains(id));
                self.resources.host_cpus.truncate(usize::from(n));
                for id in preferred {
                    if self.resources.host_cpus.len() >= usize::from(n) {
                        break;
                    }
                    if !self.resources.host_cpus.contains(&id) {
                        self.resources.host_cpus.push(id);
                    }
                }
            }
            Change::GuestNuma(topology) => {
                self.resources.guest_numa = topology;
                let step = topology.alignment(self.resources.pages);
                self.memory_mib = self.memory_mib.div_ceil(step) * step;
                self.memory_gib = size_text(u64::from(self.memory_mib));
            }
            Change::Network(mode) => self.resources.network = mode,
            Change::Memory(n) => {
                self.memory_mib = n;
                self.memory_gib = size_text(u64::from(n));
            }
            Change::MemoryText(value) => {
                if size_input(&value) {
                    self.memory_mib = parse_size(&value, 1 << 20).unwrap_or(0) as u32;
                    self.memory_gib = value;
                }
            }
            Change::Disk(n) if !self.storage_fixed => {
                self.disk_mib = u64::from(n);
                self.disk_gib = size_text(self.disk_mib);
            }
            Change::DiskText(value) if !self.storage_fixed => {
                if size_input(&value) {
                    self.disk_mib = parse_disk(&value).unwrap_or(0);
                    self.disk_gib = value;
                }
            }
            Change::PinCpus if host.cpu_pinning => {
                self.resources.pin_cpus = !self.resources.pin_cpus;
                self.resources.cpu_power_management = self.recommended_cpu_waits(host);
                if self.resources.pin_cpus {
                    self.resources.host_cpus = host.preferred_cpus(self.cpus);
                }
            }
            Change::CpuPowerManagement
                if self.resources.cpu_power_management || self.recommended_cpu_waits(host) =>
            {
                self.resources.cpu_power_management = !self.resources.cpu_power_management;
            }
            Change::PinMemory if host.memory_pinning || self.resources.pin_memory => {
                self.resources.pin_memory = !self.resources.pin_memory
            }
            Change::Reserve if !self.storage_fixed => {
                self.resources.reserve_storage = !self.resources.reserve_storage
            }
            Change::Page(page) if host.huge_supported(page) => {
                self.resources.pages = page;
                self.memory_mib = self
                    .memory_mib
                    .div_ceil(self.resources.guest_numa.alignment(page))
                    * self.resources.guest_numa.alignment(page);
                self.memory_gib = size_text(u64::from(self.memory_mib));
            }
            Change::Core(id) if self.resources.pin_cpus && host.cpu_pinning => {
                if let Some(index) = self.resources.host_cpus.iter().position(|c| *c == id) {
                    if self.resources.host_cpus.len() > usize::from(self.minimum.cpus) {
                        self.resources.host_cpus.remove(index);
                    }
                } else if self.resources.host_cpus.len() < 128
                    && host.cores.iter().any(|core| core.threads.contains(&id))
                {
                    self.resources.host_cpus.push(id);
                }
                self.cpus = self.resources.host_cpus.len() as u16;
            }
            Change::Placement(kind) if self.resources.pin_cpus && host.cpu_pinning => {
                let ids = placement(host, kind, self.cpus);
                if ids.len() >= usize::from(self.minimum.cpus) {
                    self.cpus = ids.len() as u16;
                    self.resources.host_cpus = ids;
                }
            }
            Change::ShowCores => self.show_cores = !self.show_cores,
            Change::ShowStorage => self.show_storage = !self.show_storage,
            Change::Defaults => {
                let mut fresh = Self::new(Some(host), self.minimum);
                fresh.accel = self.accel;
                fresh.vendor = self.vendor;
                fresh.resources.cpu_power_management = fresh.recommended_cpu_waits(host);
                if self.storage_fixed {
                    fresh.disk_mib = self.disk_mib;
                    fresh.disk_gib = self.disk_gib.clone();
                    fresh.resources.reserve_storage = self.resources.reserve_storage;
                    fresh.storage_fixed = true;
                }
                *self = fresh;
            }
            _ => {}
        }
    }
}
fn size_text(mib: u64) -> String {
    (mib as f64 / 1024.).to_string()
}
fn size_input(value: &str) -> bool {
    value.len() <= 16 && value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}
fn parse_size(value: &str, maximum_mib: u64) -> Option<u64> {
    let gib: f64 = value.parse().ok()?;
    (gib.is_finite() && gib > 0. && gib <= maximum_mib as f64 / 1024.)
        .then(|| (gib * 1024.).ceil() as u64)
}
fn parse_disk(value: &str) -> Option<u64> {
    parse_size(value, vkored::hardware::MAX_DISK_MIB)
}
/// Common capacities (powers of two and their halfway points), filtered by
/// the current host and image limits. Inputs remain available between presets.
fn size_presets(minimum: u64, maximum: u64, alignment: u64) -> Vec<u32> {
    let maximum = maximum.min(vkored::hardware::MAX_DISK_MIB);
    let mut sizes = Vec::new();
    let mut base = 2 * 1024u64;
    while base <= maximum {
        for size in [base, base + base / 2] {
            if size >= minimum && size <= maximum && size.is_multiple_of(alignment) {
                sizes.push(size as u32);
            }
        }
        base *= 2;
    }
    sizes
}
// Placement edits only the draft. Host admission and the launch mapping remain
// owned by vkored. Keep physical-core spreading from the shared host snapshot.
fn placement(host: &HostResources, kind: Option<&str>, count: u16) -> Vec<u32> {
    host.preferred_cpus(u16::MAX)
        .into_iter()
        .filter(|id| {
            kind.is_none_or(|kind| {
                host.cores
                    .iter()
                    .any(|core| core.kind == kind && core.threads.contains(id))
            })
        })
        .take(usize::from(count))
        .collect()
}
/// A choice among alternatives, lifted on hover; the chosen one is tinted.
fn action(
    ty: Typography,
    label: &str,
    message: Option<Message>,
    selected: bool,
) -> UiElement<'static> {
    let kind = widgets::Button::Choice(selected);
    kit::lift(
        kind,
        message.is_some(),
        button(ty.body(label.to_owned(), 13., theme::TEXT))
            .padding([7, 10])
            .style(widgets::button_style_fading(kind))
            .on_press_maybe(message),
    )
}
/// A block that calls something out: a glyph, then its text, on a faint wash
/// of the tone it speaks in.
fn callout<'a>(
    ty: Typography,
    icon: Icon,
    tone: theme::Tone,
    content: impl Into<UiElement<'a>>,
) -> UiElement<'a> {
    container(
        row![
            container(visuals::icon(icon, tone, ty.size(14.))).padding([2, 0]),
            container(content).width(Length::Fill),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Top),
    )
    .padding([theme::SPACE_2, theme::SPACE_2 + 2.])
    .width(Length::Fill)
    .style(move |appearance| theme::tinted(appearance, tone, 0.09, 0.22, theme::RADIUS_MD))
    .into()
}
/// An explanation: small, muted, led by an info glyph.
fn note(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    row![
        container(visuals::icon(Icon::Info, theme::TEXT_MUTED, ty.size(13.))).padding([2, 0]),
        ty.body(text.into(), 12., theme::TEXT_MUTED)
            .width(Length::Fill),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Top)
    .into()
}
/// Something to be careful about.
fn caution(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    callout(
        ty,
        Icon::Alert,
        theme::WARNING,
        ty.body(text.into(), 12., theme::WARNING),
    )
}
/// Something that has to be fixed.
fn problem(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    callout(
        ty,
        Icon::Alert,
        theme::ERROR,
        ty.body(text.into(), 12., theme::ERROR),
    )
}
/// A caution whose words come from outside (the host, a driver): they go
/// through the data face.
fn caution_data<'a>(ty: Typography, text: impl Into<std::borrow::Cow<'a, str>>) -> UiElement<'a> {
    callout(
        ty,
        Icon::Alert,
        theme::WARNING,
        ty.data(text, theme::WARNING)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    )
}
fn switch(
    ty: Typography,
    label: &'static str,
    enabled: bool,
    available: bool,
    message: Message,
) -> UiElement<'static> {
    toggler(enabled)
        .label(label)
        .font(ty.font())
        .text_size(ty.size(13.))
        .size(20)
        .spacing(10)
        .style(widgets::resource_toggle)
        .on_toggle_maybe(available.then_some(move |_| message.clone()))
        .into()
}
/// A section's surface: a recess in the editor's card.
fn section_style(appearance: &iced_core::Theme) -> iced_widget::container::Style {
    theme::panel(
        appearance,
        theme::PANEL_FILE,
        Some(theme::SEPARATOR),
        theme::RADIUS_LG,
    )
}
fn section<'a>(content: impl Into<UiElement<'a>>) -> UiElement<'a> {
    container(content)
        .padding(16)
        .width(Length::Fill)
        .style(section_style)
        .into()
}
/// What a section is about, as the glyph that leads its title.
fn section_icon(title: &str) -> Icon {
    match title {
        "Networking" => Icon::Network,
        "Processors" => Icon::Cpu,
        "Memory" => Icon::Memory,
        "Storage" => Icon::Volume,
        _ => Icon::Plugin,
    }
}
/// A section's title with its glyph on an accent plate.
fn title(ty: Typography, text: &'static str) -> UiElement<'static> {
    let plate = ty.size(30.);
    row![
        container(visuals::icon(
            section_icon(text),
            theme::MAGENTA,
            plate * 0.55
        ))
        .center(plate)
        .style(|appearance| {
            theme::tinted(appearance, theme::MAGENTA, 0.13, 0.30, theme::RADIUS_MD)
        }),
        ty.body(text, 17., theme::TEXT),
    ]
    .spacing(theme::SPACE_2 + 2.)
    .align_y(alignment::Vertical::Center)
    .into()
}
/// A section's title and its current value, as a chip at the far edge.
fn heading(ty: Typography, text: &'static str, value: String) -> UiElement<'static> {
    row![
        title(ty, text),
        space::horizontal(),
        kit::chip(ty, section_icon(text), value, theme::TEXT),
    ]
    .align_y(alignment::Vertical::Center)
    .spacing(12)
    .into()
}
pub fn view<'a>(
    draft: &'a Draft,
    host: &'a Result<HostResources, String>,
    editing: bool,
    busy: bool,
    ty: Typography,
) -> UiElement<'a> {
    let send = move |c| Message::Machine(MachineMessage::Hardware(editing, c));
    let Ok(host) = host else {
        return ty.data(host.as_ref().unwrap_err(), theme::ERROR).into();
    };
    let max_cpu = (host.logical_cpus.min(128) as u16).max(draft.minimum.cpus);
    let mut cpu = column![
        heading(ty, "Processors", format!("{} guest CPUs", draft.cpus)),
        ty.data(&host.cpu_name, theme::TEXT_SECONDARY),
        row![
            kit::chip(
                ty,
                Icon::Cpu,
                format!("{} physical cores", host.physical_cores),
                theme::TEXT_SECONDARY
            ),
            kit::chip(
                ty,
                Icon::Activity,
                format!("{} logical CPUs", host.logical_cpus),
                theme::TEXT_SECONDARY
            ),
        ]
        .spacing(theme::SPACE_2)
        .wrap(),
    ]
    .spacing(12);
    if !host.core_summary.is_empty() {
        cpu = cpu.push(ty.data(&host.core_summary, theme::TEXT_MUTED));
    }
    cpu = cpu
        .push(
            row![
                kit::icon_button(
                    ty,
                    Icon::Minus,
                    "Fewer CPUs",
                    widgets::Button::Secondary,
                    (!busy && draft.cpus > draft.minimum.cpus)
                        .then(|| send(Change::Cpus(draft.cpus.saturating_sub(1)))),
                ),
                slider(draft.minimum.cpus..=max_cpu, draft.cpus, move |n| send(
                    Change::Cpus(n)
                ))
                .step(1u16)
                .style(widgets::resource_slider),
                kit::icon_button(
                    ty,
                    Icon::Plus,
                    "More CPUs",
                    widgets::Button::Secondary,
                    (!busy && draft.cpus < max_cpu)
                        .then(|| send(Change::Cpus(draft.cpus.saturating_add(1)))),
                ),
            ]
            .spacing(14)
            .align_y(alignment::Vertical::Center),
        )
        .push(row![
            ty.body(
                format!("Minimum {}", draft.minimum.cpus),
                12.,
                theme::TEXT_MUTED
            ),
            space::horizontal(),
            ty.body(
                format!("{} available", host.logical_cpus),
                12.,
                theme::TEXT_MUTED
            ),
        ])
        .push(switch(
            ty,
            "Pin guest CPUs",
            draft.resources.pin_cpus,
            host.cpu_pinning && !busy,
            send(Change::PinCpus),
        ));
    cpu = cpu.push(switch(
        ty, "Native CPU waits (MONITOR/MWAIT)", draft.resources.cpu_power_management,
        !busy && (draft.resources.cpu_power_management
            || draft.recommended_cpu_waits(host)),
        send(Change::CpuPowerManagement),
    )).push(note(ty, "Enabled automatically with KVM and CPU pinning when supported. Also lets the guest manage idle states; other work sharing these cores may see higher latency."));
    if let Err(reason) = &host.cpu_power_management {
        cpu = cpu.push(ty.data(reason, theme::TEXT_MUTED));
    }
    if host.cpu_pinning {
        cpu = cpu.push(note(ty, "Keep each guest CPU on a chosen host thread. Pinning does not reserve the core exclusively."));
        if draft.resources.pin_cpus {
            cpu = cpu.push(action(
                ty,
                if draft.show_cores {
                    "Hide core map"
                } else {
                    "Choose cores…"
                },
                (!busy).then(|| send(Change::ShowCores)),
                draft.show_cores,
            ));
            if draft.show_cores {
                let mut presets = row![action(
                    ty,
                    "Spread across cores",
                    (!busy).then(|| send(Change::Placement(None))),
                    false
                )]
                .spacing(6);
                for (kind, label) in [
                    ("Performance", "Performance only"),
                    ("Efficiency", "Efficiency only"),
                ] {
                    if host.cores.iter().any(|core| core.kind == kind) {
                        presets = presets.push(action(
                            ty,
                            label,
                            (!busy
                                && placement(host, Some(kind), 128).len()
                                    >= usize::from(draft.minimum.cpus))
                            .then(|| send(Change::Placement(Some(kind)))),
                            false,
                        ));
                    }
                }
                cpu = cpu.push(presets.wrap())
                    .push(note(ty, format!("Select threads to adjust the CPU count. Keep at least {} selected. Siblings share one physical core.", draft.minimum.cpus)));
                let mut groups = std::collections::BTreeMap::new();
                for core in &host.cores {
                    groups
                        .entry((
                            core.package,
                            match core.kind.as_str() {
                                "Performance" => 0,
                                "Efficiency" => 1,
                                _ => 2,
                            },
                            core.kind.as_str(),
                        ))
                        .or_insert_with(Vec::new)
                        .push(core);
                }
                let mut map = column![].spacing(14);
                for ((socket, _, kind), cores) in groups {
                    let mut tiles = row![].spacing(8);
                    for core in cores {
                        let selected = core
                            .threads
                            .iter()
                            .filter(|id| draft.resources.host_cpus.contains(id))
                            .count();
                        let mut threads = row![].spacing(4);
                        for &id in &core.threads {
                            let active = draft.resources.host_cpus.contains(&id);
                            let can_change = if active {
                                draft.resources.host_cpus.len() > usize::from(draft.minimum.cpus)
                            } else {
                                draft.resources.host_cpus.len() < 128
                            };
                            threads = threads.push(action(
                                ty,
                                &format!("CPU {id}"),
                                (!busy && can_change).then(|| send(Change::Core(id))),
                                active,
                            ));
                        }
                        tiles = tiles.push(
                            container(
                                column![
                                    row![
                                        ty.body(format!("Core {}", core.id), 13., theme::TEXT),
                                        space::horizontal(),
                                        ty.body(
                                            format!("{selected}/{}", core.threads.len()),
                                            11.,
                                            theme::TEXT_MUTED
                                        )
                                    ]
                                    .spacing(6),
                                    threads.wrap(),
                                ]
                                .spacing(9),
                            )
                            .padding(10)
                            .width(158)
                            .style(move |appearance| {
                                if selected > 0 {
                                    theme::tinted(
                                        appearance,
                                        theme::MAGENTA,
                                        0.08,
                                        0.35,
                                        theme::RADIUS_MD,
                                    )
                                } else {
                                    theme::panel(
                                        appearance,
                                        theme::PANEL,
                                        Some(theme::SEPARATOR),
                                        theme::RADIUS_MD,
                                    )
                                }
                            }),
                        );
                    }
                    map = map.push(
                        column![
                            ty.data(format!("{kind} · socket {socket}"), theme::TEXT_SECONDARY),
                            tiles.wrap()
                        ]
                        .spacing(8),
                    );
                }
                cpu = cpu.push(
                    widgets::scroll(
                        container(map).padding(iced_core::Padding::default().right(16)),
                    )
                    .width(Length::Fill)
                    .height(Length::Shrink.max(340)),
                );
            }
            if draft.resources.host_cpus.len() != usize::from(draft.cpus) {
                cpu = cpu.push(caution(ty, "This saved selection no longer matches the CPU count. Choose a placement preset or adjust the count."));
            }
        }
    } else {
        cpu = cpu.push(note(ty, "macOS manages core placement. You can adjust the CPU count; exact pinning is unavailable."));
    }
    container(
        column![
            row![
                kit::icon_label(
                    ty,
                    Icon::Check,
                    "Use recommended settings",
                    widgets::Button::Secondary,
                    (!busy).then(|| send(Change::Defaults)),
                ),
                kit::icon_label(
                    ty,
                    Icon::Refresh,
                    "Refresh host",
                    widgets::Button::Secondary,
                    (!busy).then_some(Message::Machine(MachineMessage::RefreshHardware)),
                ),
            ]
            .spacing(8)
            .wrap(),
            section(column![
                title(ty, "Networking"),
                row([NetworkMode::Isolated, NetworkMode::E1000e].map(|mode| kit::icon_label(
                    ty,
                    match mode {
                        NetworkMode::Isolated => Icon::Lock,
                        NetworkMode::E1000e => Icon::Network,
                    },
                    mode.label(),
                    widgets::Button::Choice(draft.resources.network == mode),
                    (!busy).then(|| send(Change::Network(mode))),
                )))
                .spacing(6)
                .wrap(),
                note(ty, match draft.resources.network {
                        NetworkMode::Isolated => "Guest traffic stays inside the virtual network.",
                        NetworkMode::E1000e => "Uses the host network for outbound IPv4. The guest image needs IPv4 support.",
                    }),
            ].spacing(12)),
            section(cpu),
            memory_view(draft, host, editing, busy, ty),
            section(npu_inventory(
                &host.npus,
                &draft.resources.passthrough,
                editing,
                busy,
                ty
            )),
            section(gpu_inventory(draft, host, editing, busy, ty)),
            storage_view(draft, host, editing, busy, ty),
        ]
        .spacing(16),
    )
    .padding(iced_core::Padding::default().right(14))
    .width(Length::Fill)
    .into()
}

pub(crate) fn memory_view<'a>(
    draft: &'a Draft,
    host: &'a HostResources,
    editing: bool,
    busy: bool,
    ty: Typography,
) -> UiElement<'a> {
    let send = move |c| Message::Machine(MachineMessage::Hardware(editing, c));
    let max_ram = (host.memory_mib.min(1 << 20) as u32).max(draft.minimum.memory_mib);
    let ram_step = draft.resources.guest_numa.alignment(draft.resources.pages);
    let mut memory_presets = row![].spacing(6);
    for mib in size_presets(
        u64::from(draft.minimum.memory_mib),
        host.memory_mib.min(1 << 20),
        u64::from(ram_step),
    ) {
        memory_presets = memory_presets.push(action(
            ty,
            &format!("{} GiB", mib / 1024),
            (!busy).then(|| send(Change::Memory(mib))),
            draft.memory_mib == mib,
        ));
    }
    let mut memory = column![
        heading(
            ty,
            "Memory",
            format!("{:.2} GiB", f64::from(draft.memory_mib) / 1024.)
        ),
        row![
            ty.body("Allocation", 13., theme::TEXT),
            text_input("8", &draft.memory_gib)
                .on_input_maybe((!busy).then_some(move |s| send(Change::MemoryText(s))))
                .font(ty.font())
                .size(ty.size(14.))
                .padding([7, 10])
                .width(110)
                .style(widgets::field),
            ty.body("GiB", 13., theme::TEXT_MUTED),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center),
        memory_presets.width(Length::Fill).wrap(),
        slider(
            draft.minimum.memory_mib..=max_ram,
            draft.memory_mib.clamp(draft.minimum.memory_mib, max_ram),
            move |n| send(Change::Memory(n / ram_step * ram_step))
        )
        .step(ram_step)
        .style(widgets::resource_slider),
        row![
            kit::chip(
                ty,
                Icon::Memory,
                format!("{:.1} GiB physical", host.memory_mib as f64 / 1024.),
                theme::TEXT_SECONDARY
            ),
            kit::chip(
                ty,
                Icon::Check,
                format!("{:.1} GiB free", host.free_memory_mib as f64 / 1024.),
                theme::TEXT_SECONDARY
            ),
            kit::chip(
                ty,
                Icon::Info,
                format!(
                    "{:.1} GiB reclaimable (OS estimate)",
                    host.available_memory_mib as f64 / 1024.
                ),
                theme::TEXT_SECONDARY
            ),
        ]
        .spacing(theme::SPACE_2)
        .wrap(),
        switch(
            ty,
            "Pin memory in RAM",
            draft.resources.pin_memory,
            (host.memory_pinning || draft.resources.pin_memory) && !busy,
            send(Change::PinMemory)
        ),
    ]
    .spacing(12)
    .width(Length::Fill);
    if draft.memory_mib < draft.minimum.memory_mib
        || u64::from(draft.memory_mib) > host.memory_mib.min(1 << 20)
    {
        memory = memory.push(problem(
            ty,
            format!(
                "Enter {:.2}–{:.2} GiB, within the image minimum and host RAM.",
                f64::from(draft.minimum.memory_mib) / 1024.,
                host.memory_mib.min(1 << 20) as f64 / 1024.
            ),
        ));
    } else if !draft.memory_mib.is_multiple_of(ram_step) {
        memory = memory.push(problem(
            ty,
            format!("This page size and memory layout require multiples of {ram_step} MiB."),
        ));
    }
    if draft.resources.pages == PageSize::System
        && u64::from(draft.memory_mib) > host.available_memory_mib
    {
        memory = memory.push(caution(ty, if draft.resources.pin_memory {
            "Not enough available RAM to pin this allocation. Reduce it or close other workloads."
        } else { "This exceeds currently available RAM. The host may swap; reduce the allocation or close other workloads." }));
    }
    if !host.memory_pinning {
        memory = memory.push(note(
            ty,
            "Memory locking is unavailable in QEMU on this host.",
        ));
    } else if (!draft.resources.passthrough.is_empty()
        || !draft.resources.gpu_vfs.is_empty()
        || draft.resources.pin_memory)
        && host.memlock_bytes.is_some_and(|n| {
            vkored::hardware::lock_budget(
                draft.memory_mib,
                draft.resources.passthrough.len() + draft.resources.gpu_vfs.len(),
            )
            .is_ok_and(|budget| n < budget)
        })
    {
        memory = memory.push(note(
            ty,
            "Permissions are checked when you finish. System authorization appears only if needed.",
        ));
    }
    memory = memory.push(kit::heading(ty, Icon::Dashboard, "Guest memory layout"))
        .push(row![
            action(ty, "Single node", (!busy).then(|| send(Change::GuestNuma(GuestNuma::Single))), draft.resources.guest_numa == GuestNuma::Single),
            action(ty, "Two nodes", (!busy).then(|| send(Change::GuestNuma(GuestNuma::Split))), draft.resources.guest_numa == GuestNuma::Split),
        ].spacing(6).wrap())
        .push(note(ty, "Single node keeps all guest RAM together for inference. Two nodes preserves the Kore gate topology and divides RAM equally."));
    memory = memory.push(kit::heading(ty, Icon::Memory, "Preferred host pages"));
    let mut pages = row![action(
        ty,
        &format!("Automatic · {} KiB", host.system_page_bytes / 1024),
        (!busy).then(|| send(Change::Page(PageSize::System))),
        draft.resources.pages == PageSize::System,
    )]
    .spacing(6);
    if host.hugetlb_supported {
        for page in [PageSize::Huge2M, PageSize::Huge1G] {
            pages = pages.push(action(
                ty,
                page.label(),
                (!busy && host.huge_supported(page)).then(|| send(Change::Page(page))),
                draft.resources.pages == page,
            ));
        }
        memory = memory.push(pages.wrap()).push(note(ty, "At launch, vKore compacts memory and tries this page size. If full coverage is unavailable, it uses 2 MiB transparent hugepages where possible and system pages for the rest. Your RAM size and guest layout stay unchanged; boot activity reports the result."));
        for page in [PageSize::Huge2M, PageSize::Huge1G] {
            if !host.huge_available(page, draft.memory_mib) {
                let reason = match host.hugepages.iter().find(|pool| pool.page == page) {
                    Some(pool) => format!(
                        "{}: {:.1} GiB available in the pool; {:.1} GiB needed. Partial coverage is allowed at launch.",
                        page.label(),
                        pool.free_mib as f64 / 1024.,
                        f64::from(draft.memory_mib) / 1024.
                    ),
                    None => format!("{}: not supported by this host kernel.", page.label()),
                };
                memory = memory.push(note(ty, reason));
            }
        }
    } else {
        memory = memory.push(pages.wrap()).push(note(ty, "Automatic uses this host's native pages. QEMU's explicit 2 MiB and 1 GiB hugepage backing requires Linux; it is unavailable on this host."));
        if draft.resources.pages != PageSize::System {
            memory = memory.push(caution(
                ty,
                format!(
                    "Saved choice: {}. Choose Automatic to use this machine on this host.",
                    draft.resources.pages.label()
                ),
            ));
        }
    }
    memory = memory.push(note(
        ty,
        "This controls host memory backing, not page sizes inside Kore.",
    ));
    for node in &host.memory_nodes {
        memory = memory.push(ty.data(
            format!(
                "NUMA {} · {:.1} GiB · CPUs {}",
                node.id,
                node.memory_mib as f64 / 1024.,
                node.cpus
            ),
            theme::TEXT_MUTED,
        ));
    }
    container(memory)
        .id("hardware-memory")
        .padding(16)
        .width(Length::Fill)
        .style(section_style)
        .into()
}

pub(crate) fn storage_view<'a>(
    draft: &'a Draft,
    host: &'a HostResources,
    editing: bool,
    busy: bool,
    ty: Typography,
) -> UiElement<'a> {
    let send = move |c| Message::Machine(MachineMessage::Hardware(editing, c));
    let max_disk = vkored::hardware::disk_limit(host, draft.resources.reserve_storage)
        .max(draft.minimum.disk_mib) as u32;
    let mut storage = column![heading(
        ty,
        "Storage",
        format!("{:.1} GiB", draft.disk_mib as f64 / 1024.)
    )]
    .spacing(12);
    if draft.storage_fixed {
        storage = storage.push(note(
            ty,
            "Disk size and reservation are fixed after first launch.",
        ));
    } else {
        let minimum = draft.minimum.disk_mib as u32;
        // A logarithmic scale keeps small allocations usable on multi-TiB hosts.
        let low = (f64::from(minimum) / 1024.).log2();
        let high = (f64::from(max_disk) / 1024.).log2();
        let mut presets = row![].spacing(6);
        for mib in size_presets(
            u64::from(minimum).max(16 * 1024),
            vkored::hardware::disk_limit(host, draft.resources.reserve_storage),
            1,
        ) {
            let gib = mib / 1024;
            presets = presets.push(action(
                ty,
                &format!("{gib} GiB"),
                (!busy && mib >= minimum && mib <= max_disk).then(|| send(Change::Disk(mib))),
                draft.disk_mib == u64::from(mib),
            ));
        }
        storage = storage
            .push(row![
                ty.body("Capacity", 13., theme::TEXT),
                text_input("32", &draft.disk_gib)
                    .on_input_maybe((!busy).then_some(move |s| send(Change::DiskText(s))))
                    .font(ty.font()).size(ty.size(14.)).padding([7, 10])
                    .width(110).style(widgets::field),
                ty.body("GiB", 13., theme::TEXT_MUTED),
            ].spacing(10).align_y(alignment::Vertical::Center))
            .push(slider(low..=high, (draft.disk_mib.max(1) as f64 / 1024.).log2().clamp(low, high),
                move |n| send(Change::Disk(((n.exp2().round() as u32) * 1024).clamp(minimum, max_disk))))
                .step(0.01).style(widgets::resource_slider))
            .push(row![
                ty.body(format!("Minimum {:.1} GiB", f64::from(minimum) / 1024.), 12., theme::TEXT_MUTED),
                space::horizontal(),
                ty.body(format!("Up to {:.1} GiB", f64::from(max_disk) / 1024.), 12., theme::TEXT_MUTED),
            ])
            .push(presets.wrap())
            .push(note(ty, "Allow room for model weights and working files. Size can be changed until first launch."))
            .push(switch(ty, "Reserve disk space", draft.resources.reserve_storage, !busy, send(Change::Reserve)));
        if draft.disk_mib < draft.minimum.disk_mib
            || draft.disk_mib > vkored::hardware::MAX_DISK_MIB
        {
            storage = storage.push(problem(
                ty,
                format!(
                    "Enter a capacity from {:.1} to 4096 GiB.",
                    draft.minimum.disk_mib as f64 / 1024.
                ),
            ));
        } else if draft.disk_mib > host.storage_available_mib {
            storage = storage.push(caution(ty, if draft.resources.reserve_storage {
                "Not enough free space to reserve this disk. Reduce its size, free space, or turn off reservation."
            } else {
                "This sparse disk can grow beyond currently free space. Free more space before filling it; guest writes can fail if the host runs out."
            }));
        }
    }
    storage = storage
        .push(kit::chip(
            ty,
            Icon::Check,
            format!(
                "{:.1} GiB free on the storage filesystem",
                host.storage_available_mib as f64 / 1024.
            ),
            theme::TEXT_SECONDARY,
        ))
        .push(note(
            ty,
            if draft.resources.reserve_storage {
                "Reserves space at first launch and requests contiguous placement where supported."
            } else {
                "Sparse allocation uses space as data is written; capacity is not reserved."
            },
        ))
        .push(action(
            ty,
            if draft.show_storage {
                "Hide storage details"
            } else {
                "Storage details"
            },
            (!busy).then(|| send(Change::ShowStorage)),
            draft.show_storage,
        ));
    if draft.show_storage {
        storage = storage.push(ty.data(
            host.storage_path.display().to_string(),
            theme::TEXT_SECONDARY,
        ));
        for disk in &host.storage {
            storage = storage.push(ty.data(
                format!(
                    "{} · {} · {} · {:.1} GiB free / {:.1} GiB · {}",
                    disk.device,
                    disk.kind,
                    disk.filesystem,
                    disk.available_mib as f64 / 1024.,
                    disk.total_mib as f64 / 1024.,
                    disk.mount.display()
                ),
                theme::TEXT_MUTED,
            ));
        }
    }
    section(storage)
}

pub(crate) fn gpu_inventory<'a>(
    draft: &'a Draft,
    host: &'a HostResources,
    editing: bool,
    busy: bool,
    ty: Typography,
) -> iced_widget::Column<'a, Message, iced_core::Theme, crate::AxUi> {
    use vkore_vm::passthrough::gpu::{Assignment, eligible_parent};
    let send = move |selection, enabled| {
        Message::Machine(MachineMessage::Hardware(
            editing,
            Change::Gpu(selection, enabled),
        ))
    };
    let mut content = column![title(ty, "Graphics processors"),
        note(ty, "Give this guest a GPU virtual function while the host keeps its display. Managed SR-IOV currently supports Linux Intel xe GPUs.")].spacing(12);
    for warning in &host.gpus.warnings {
        content = content.push(caution_data(ty, warning));
    }
    let parents: Vec<_> = host
        .gpus
        .devices
        .iter()
        .filter(|g| g.pci.physical_function.is_none())
        .collect();
    if parents.is_empty() {
        content = content.push(note(
            ty,
            if cfg!(target_os = "linux") {
                "No PCI GPUs were reported. Refresh the host after configuring its GPU driver."
            } else {
                "GPU SR-IOV assignment is available on Linux."
            },
        ));
    }
    for gpu in parents {
        let saved = draft
            .resources
            .gpu_vfs
            .iter()
            .find(|s| s.physical_function == gpu.pci.address);
        let selection = saved.cloned().unwrap_or_else(|| Assignment::new(gpu, 0));
        let eligible = eligible_parent(gpu);
        let enabled = saved.is_some();
        let capacity = gpu.pci.total_vfs.unwrap_or(0).min(64);
        let count = gpu.pci.enabled_vfs.unwrap_or(0);
        let limit = if count == 0 { capacity } else { count.min(64) };
        let mut card = column![
            ty.data(&gpu.name, theme::TEXT).size(ty.size(15.)),
            ty.data(
                format!(
                    "{} · {} · {count} VFs enabled / {capacity} available",
                    gpu.pci.address,
                    gpu.driver.as_deref().unwrap_or("No driver bound")
                ),
                theme::TEXT_MUTED
            ),
            switch(
                ty,
                "Use a GPU virtual function",
                enabled,
                !busy && (enabled || eligible.is_ok()),
                send(selection.clone(), !enabled)
            )
        ]
        .spacing(8);
        if enabled {
            let mut previous = selection.clone();
            previous.index = previous.index.saturating_sub(1);
            let mut next = selection.clone();
            next.index += 1;
            card = card.push(
                row![
                    kit::icon_button(
                        ty,
                        Icon::Minus,
                        "Previous virtual function",
                        widgets::Button::Secondary,
                        (!busy && selection.index > 0).then(|| send(previous, true)),
                    ),
                    ty.body(
                        format!("Virtual function {}", selection.index + 1),
                        13.,
                        theme::TEXT
                    ),
                    kit::icon_button(
                        ty,
                        Icon::Plus,
                        "Next virtual function",
                        widgets::Button::Secondary,
                        (!busy && selection.index + 1 < limit).then(|| send(next, true)),
                    )
                ]
                .spacing(10)
                .align_y(alignment::Vertical::Center),
            );
            if count == 0 {
                card = card.push(ty.body(format!("Done will create {} virtual function(s) and use VF {}. The GPU driver allocates their resources. The host GPU stays attached.", selection.index + 1, selection.index + 1), 12., theme::TEXT_SECONDARY));
            } else {
                match selection.resolve(&host.gpus) {
                    Ok(vf) => {
                        card = card.push(ty.data(
                            format!(
                                "Assign {} · dedicated to this guest while running",
                                vf.address
                            ),
                            theme::TEXT_SECONDARY,
                        ))
                    }
                    Err(reason) => card = card.push(caution_data(ty, reason)),
                }
            }
            card = card.push(note(ty, "Permissions are requested on Done/Save and launch. Created VFs remain on the host until reboot; finish setup again to recreate them. Existing pools are never resized or removed."));
        }
        if let Err(reason) = eligible {
            card = card.push(ty.data(reason, theme::TEXT_MUTED));
        }
        content = content.push(
            container(card)
                .padding(14)
                .width(Length::Fill)
                .style(widgets::card),
        );
    }
    for saved in &draft.resources.gpu_vfs {
        if !host
            .gpus
            .devices
            .iter()
            .any(|g| g.pci.address == saved.physical_function)
        {
            content = content.push(
                column![
                    caution_data(
                        ty,
                        format!("Saved GPU {} is missing.", saved.physical_function)
                    ),
                    switch(
                        ty,
                        "Use a GPU virtual function",
                        true,
                        !busy,
                        send(saved.clone(), false)
                    )
                ]
                .spacing(8),
            );
        }
    }
    content
}

pub(crate) fn npu_inventory<'a>(
    inventory: &'a vkore_vm::devices::Inventory,
    selected: &'a [vkore_vm::passthrough::Selection],
    editing: bool,
    busy: bool,
    ty: Typography,
) -> iced_widget::Column<'a, Message, iced_core::Theme, crate::AxUi> {
    use vkore_vm::devices::DiscoveryStatus;
    let mut content = column![
        row![
            title(ty, "Neural processors"),
            space::horizontal(),
            kit::chip(
                ty,
                match inventory.status {
                    DiscoveryStatus::Complete => Icon::Check,
                    DiscoveryStatus::Partial => Icon::Alert,
                    DiscoveryStatus::Unavailable => Icon::Info,
                },
                match inventory.status {
                    DiscoveryStatus::Complete => format!("{} detected", inventory.devices.len()),
                    DiscoveryStatus::Partial =>
                        format!("{} reported · incomplete", inventory.devices.len()),
                    DiscoveryStatus::Unavailable => "Discovery unavailable".into(),
                },
                theme::TEXT_SECONDARY
            )
        ]
        .align_y(alignment::Vertical::Center),
        note(ty, "Assign a Linux NPU exclusively to this guest. Shared accelerator connections are not available yet."),
    ]
    .spacing(12);
    if inventory.devices.is_empty() {
        content = content.push(ty.body(
            match inventory.status {
                DiscoveryStatus::Complete => "No NPUs were reported. Refresh the host after connecting a device or installing its driver.",
                DiscoveryStatus::Partial => "Discovery was incomplete; an NPU may be present but could not be identified.",
                DiscoveryStatus::Unavailable => "NPU discovery is unavailable. CPU, memory and storage settings remain usable.",
            }, 13., theme::TEXT_SECONDARY));
    }
    for warning in &inventory.warnings {
        content = content.push(caution_data(ty, warning));
    }
    for device in &inventory.devices {
        let mut card = column![
            ty.data(&device.name, theme::TEXT).size(ty.size(15.)),
            ty.data(
                format!("{} · {}", device.vendor.label(), device.source.label()),
                theme::TEXT_MUTED
            ),
        ]
        .spacing(8);
        if let Some(selection) = vkore_vm::passthrough::Selection::from_device(device) {
            let enabled = selected.contains(&selection);
            let eligibility = vkore_vm::passthrough::eligible(device);
            card = card.push(switch(
                ty,
                "Full passthrough",
                enabled,
                !busy
                    && (enabled
                        || (eligibility.is_ok()
                            && selected.len() < vkore_vm::passthrough::MAX_DEVICES)),
                Message::Machine(MachineMessage::Hardware(
                    editing,
                    Change::Passthrough(selection),
                )),
            ));
            card = card.push(note(ty, "While the guest runs, this NPU is unavailable to host apps. vKore restores its host driver after shutdown. Passthrough locks guest RAM for DMA even when memory pinning is off. Setup and launch may request system authentication."));
            if let Err(reason) = eligibility {
                card = card.push(caution_data(ty, reason));
            }
        }
        if let Some(cores) = device.core_count {
            card = card.push(ty.data(
                format!("{cores} Neural Engine cores"),
                theme::TEXT_SECONDARY,
            ));
        }
        if let Some(pci) = &device.pci {
            card = card.push(ty.data(
                format!(
                    "{} · {:04x}:{:04x} · {}",
                    pci.address,
                    pci.vendor_id,
                    pci.device_id,
                    device.driver.as_deref().unwrap_or("No driver bound")
                ),
                theme::TEXT_MUTED,
            ));
            if let Some(group) = pci.iommu_group {
                let membership = match pci.group_members.len() {
                    0 => "membership unavailable".into(),
                    1 => "1 device".into(),
                    count => format!("{count} devices"),
                };
                card = card.push(ty.data(
                    format!("IOMMU group {group} · {membership}"),
                    theme::TEXT_MUTED,
                ));
            }
            if pci.total_vfs.is_some_and(|n| n > 0) {
                card = card.push(ty.data(
                    format!(
                        "SR-IOV: {} enabled / {} total VFs",
                        pci.enabled_vfs.map_or("unknown".into(), |n| n.to_string()),
                        pci.total_vfs.unwrap()
                    ),
                    theme::TEXT_MUTED,
                ));
            }
        }
        for route in vkored::hardware::npu_routes(device) {
            card = card.push(
                column![
                    row![ty.body(route.name, 13., theme::TEXT_SECONDARY), {
                        use vkored::hardware::AccessStatus::*;
                        let (icon, tone) = match route.status {
                            Configurable => (Icon::Check, theme::SUCCESS),
                            Pending => (Icon::Info, theme::TEXT_SECONDARY),
                            NeedsSetup => (Icon::Alert, theme::WARNING),
                            Unavailable => (Icon::Close, theme::TEXT_MUTED),
                            Unknown => (Icon::Info, theme::TEXT_MUTED),
                        };
                        kit::chip(ty, icon, route.status.label(), tone)
                    }]
                    .align_y(alignment::Vertical::Center)
                    .spacing(ty.size(theme::SPACE_2))
                    .wrap(),
                    note(ty, route.detail),
                ]
                .spacing(4),
            );
        }
        content = content.push(
            container(card)
                .padding(14)
                .width(Length::Fill)
                .style(widgets::card),
        );
    }
    for selection in selected.iter().filter(|s| {
        !inventory
            .devices
            .iter()
            .any(|d| vkore_vm::passthrough::Selection::from_device(d).as_ref() == Some(*s))
    }) {
        content = content.push(column![
            caution_data(ty, format!("Saved NPU {} is missing or changed. Disable it to use this configuration on the current host.",selection.address)),
            switch(ty, "Full passthrough", true, !busy, Message::Machine(MachineMessage::Hardware(editing, Change::Passthrough(selection.clone())))),
        ].spacing(8));
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> HostResources {
        let root = tempfile::tempdir().unwrap();
        let mut host = HostResources::probe(root.path()).unwrap();
        host.cpu_pinning = true;
        host.acceleration = vkore_vm::HostCaps {
            vendor: Some(vkore_vm::Vendor::Intel),
            kvm: true,
            hvf: false,
        };
        host.logical_cpus = 12;
        host.physical_cores = 8;
        host.cores = (0..8)
            .map(|id| vkore_vm::hardware::Core {
                package: 0,
                id,
                numa_node: Some(0),
                kind: if id < 4 { "Performance" } else { "Efficiency" }.into(),
                threads: if id < 4 { vec![id, id + 8] } else { vec![id] },
            })
            .collect();
        host
    }
    #[test]
    fn native_waits_follow_pinning_and_preserve_manual_choices() {
        let mut host = host();
        host.cpu_power_management = Ok(());
        let mut draft = Draft::new(Some(&host), Requirements::default());
        assert!(draft.resources.cpu_power_management);
        draft.change(Change::PinCpus, &host);
        assert!(!draft.resources.pin_cpus);
        assert!(!draft.resources.cpu_power_management);
        draft.change(Change::PinCpus, &host);
        assert!(draft.resources.pin_cpus);
        assert!(draft.resources.cpu_power_management);
        draft.change(Change::CpuPowerManagement, &host);
        assert!(!draft.resources.cpu_power_management);
        draft.change(Change::Cpus(2), &host);
        let mut config = MachineConfig::new("waits", "/image.iso");
        draft.apply(&mut config);
        assert!(
            !Draft::from_config(&config, false)
                .resources
                .cpu_power_management
        );
        draft.change(Change::Defaults, &host);
        assert!(draft.resources.cpu_power_management);
        // A saved selection can still be turned off after moving to another host.
        host.cpu_power_management = Err("unavailable".into());
        draft.change(Change::CpuPowerManagement, &host);
        assert!(!draft.resources.cpu_power_management);
        draft.change(Change::CpuPowerManagement, &host);
        assert!(!draft.resources.cpu_power_management);
        draft.change(Change::PinCpus, &host);
        draft.change(Change::PinCpus, &host);
        assert!(!draft.resources.cpu_power_management);
    }

    #[test]
    fn defaults_and_pinning_respect_the_saved_accelerator_and_vendor() {
        let mut host = host();
        host.cpu_power_management = Ok(());
        for (accel, vendor) in [
            (vkore_vm::AccelChoice::Tcg, None),
            (vkore_vm::AccelChoice::Hvf, None),
            (vkore_vm::AccelChoice::Auto, Some(vkore_vm::Vendor::Amd)),
        ] {
            let mut config = MachineConfig::new("waits", "/image.iso");
            config.accel = accel;
            config.vendor = vendor;
            let mut draft = Draft::from_config(&config, false);
            draft.change(Change::PinCpus, &host);
            assert!(!draft.resources.cpu_power_management);
            draft.change(Change::Defaults, &host);
            assert!(!draft.resources.cpu_power_management);
            draft.change(Change::CpuPowerManagement, &host);
            assert!(!draft.resources.cpu_power_management);
            draft.apply(&mut config);
            assert_eq!(config.accel, accel);
            assert_eq!(config.vendor, vendor);
        }
    }

    #[test]
    fn presets_follow_host_limits_minimums_and_page_alignment() {
        assert_eq!(size_presets(8192, 16 * 1024, 1), vec![8192, 12288, 16384]);
        assert_eq!(
            size_presets(16 * 1024, 65 * 1024, 1),
            vec![16384, 24576, 32768, 49152, 65536]
        );
        assert_eq!(size_presets(2048, 8192, 2048), vec![2048, 4096, 6144, 8192]);
        assert!(size_presets(16384, 8192, 1).is_empty());
        let large = size_presets(16 * 1024, u64::MAX, 1);
        assert_eq!(large.last(), Some(&(vkored::hardware::MAX_DISK_MIB as u32)));
        assert!(large.windows(2).all(|pair| pair[0] < pair[1]));
    }
    #[test]
    fn memory_inputs_preserve_precise_values_and_expose_invalid_drafts() {
        let mut host = host();
        host.memory_mib = 32 * 1024;
        let mut draft = Draft::new(Some(&host), Requirements::default());
        draft.resources.pages = PageSize::System;
        draft.change(Change::Memory(12 * 1024), &host);
        assert_eq!(draft.memory_gib, "12");
        draft.change(Change::MemoryText("12.5".into()), &host);
        assert_eq!(draft.memory_mib, 12800);
        draft.change(Change::MemoryText("12.0009765625".into()), &host);
        assert_eq!(draft.memory_mib, 12289);
        let mut config = MachineConfig::new("draft", "/image.iso");
        draft.apply(&mut config);
        assert!(vkored::hardware::validate(&config).is_ok());
        // Changing topology aligns RAM and keeps the editable text in sync.
        draft.change(Change::GuestNuma(GuestNuma::Split), &host);
        assert_eq!(draft.memory_mib, 12290);
        assert_eq!(parse_size(&draft.memory_gib, 1 << 20), Some(12290));
        draft.change(Change::MemoryText("12.0009765625".into()), &host);
        draft.apply(&mut config);
        assert!(vkored::hardware::validate(&config).is_err());
        for invalid in ["", "1", "2..5", "1025"] {
            draft.change(Change::MemoryText(invalid.into()), &host);
            draft.apply(&mut config);
            assert!(vkored::hardware::validate(&config).is_err(), "{invalid}");
        }
        draft.change(Change::MemoryText("64".into()), &host);
        assert_eq!(draft.memory_mib, 65536); // No silent host-capacity clamp.
        draft.apply(&mut config);
        assert!(vkored::hardware::validate_host(&config, &host).is_err());
    }

    #[test]
    fn disk_controls_support_model_sized_and_sparse_allocations() {
        let mut host = host();
        host.storage_available_mib = 8 * 1024;
        let mut draft = Draft::new(Some(&host), Requirements::default());
        assert_eq!(draft.disk_mib, vkored::hardware::DEFAULT_DISK_MIB);
        draft.change(Change::Reserve, &host);
        assert_eq!(vkored::hardware::disk_limit(&host, false), 4 * 1024 * 1024);
        draft.change(Change::Disk(128 * 1024), &host);
        assert_eq!(draft.disk_gib, "128");
        draft.change(Change::DiskText("256.5".into()), &host);
        assert_eq!(draft.disk_mib, 256 * 1024 + 512);
        draft.change(Change::Reserve, &host);
        assert_eq!(draft.disk_mib, 256 * 1024 + 512); // Never silently shrink.
        draft.change(Change::DiskText("".into()), &host);
        let mut config = MachineConfig::new("draft", "/image.iso");
        draft.apply(&mut config);
        assert!(vkored::hardware::validate(&config).is_err());
        draft.change(Change::DiskText("4097".into()), &host);
        assert_eq!(draft.disk_mib, 0);
        draft.change(Change::DiskText("4096".into()), &host);
        assert_eq!(draft.disk_mib, 1 << 22);
        draft.storage_fixed = true;
        draft.change(Change::Disk(32 * 1024), &host);
        draft.change(Change::DiskText("64".into()), &host);
        assert_eq!(draft.disk_mib, 1 << 22);
    }
    #[test]
    fn size_text_round_trips_exact_mib_and_rejects_invalid_numbers() {
        for mib in [1024, 1537, 32768, 262145, 1 << 22] {
            assert_eq!(parse_disk(&size_text(mib)), Some(mib));
        }
        for value in ["", "0", "-1", "NaN", "inf", "2.3.4", "4096.01"] {
            assert_eq!(parse_disk(value), None);
        }
    }
    #[test]
    fn missing_saved_passthrough_can_be_disabled_and_cannot_be_reenabled() {
        let mut host = host();
        host.npus.devices.clear();
        let selected = vkore_vm::passthrough::Selection {
            address: "0000:00:0b.0".into(),
            vendor_id: 0x8086,
            device_id: 0x643e,
            physical_function: None,
        };
        let mut config = MachineConfig::new("demo", "/image.iso");
        config.resources.passthrough.push(selected.clone());
        let mut draft = Draft::from_config(&config, false);
        assert_eq!(
            draft.resources.passthrough.as_slice(),
            std::slice::from_ref(&selected)
        );
        draft.change(Change::Passthrough(selected.clone()), &host);
        assert!(draft.resources.passthrough.is_empty());
        draft.change(Change::Passthrough(selected), &host);
        assert!(draft.resources.passthrough.is_empty());
        draft.apply(&mut config);
        assert!(config.resources.passthrough.is_empty());
    }
    #[test]
    fn supported_hugepages_can_be_selected_with_an_empty_pool() {
        let mut host = host();
        host.hugetlb_supported = true;
        host.hugepages = vec![vkore_vm::hardware::HugePool {
            page: PageSize::Huge2M,
            free_mib: 0,
        }];
        let mut draft = Draft::new(Some(&host), Requirements::default());
        assert_eq!(draft.resources.pages, PageSize::System);
        draft.change(Change::Page(PageSize::Huge2M), &host);
        assert_eq!(draft.resources.pages, PageSize::Huge2M);
        draft.change(Change::Page(PageSize::Huge1G), &host);
        assert_eq!(draft.resources.pages, PageSize::Huge2M); // Unsupported size.
        let mut config = MachineConfig::new("pages", "/image.iso");
        draft.apply(&mut config);
        assert_eq!(config.resources.pages, PageSize::Huge2M);
    }
    #[test]
    fn unsupported_saved_memory_choices_can_be_disabled_without_enabling_them() {
        let mut host = host();
        host.memory_pinning = false;
        host.hugetlb_supported = false;
        host.system_page_bytes = 16384;
        let mut draft = Draft::new(Some(&host), Requirements::default());
        assert_eq!(draft.resources.pages, PageSize::System);
        assert!(!draft.resources.pin_memory);
        // A machine copied from a Linux host needs explicit recovery, not a
        // silent downgrade or an unsupported switch that cannot be turned off.
        draft.resources.pages = PageSize::Huge2M;
        draft.resources.pin_memory = true;
        draft.change(Change::Page(PageSize::System), &host);
        draft.change(Change::PinMemory, &host);
        assert_eq!(draft.resources.pages, PageSize::System);
        assert!(!draft.resources.pin_memory);
        draft.change(Change::Page(PageSize::Huge1G), &host);
        draft.change(Change::PinMemory, &host);
        assert_eq!(draft.resources.pages, PageSize::System);
        assert!(!draft.resources.pin_memory);
    }
    #[test]
    fn exact_threads_and_count_stay_in_sync_and_respect_minimum() {
        let host = host();
        let mut draft = Draft::new(Some(&host), Requirements::default());
        draft.change(Change::Placement(Some("Performance")), &host);
        assert_eq!(draft.resources.host_cpus, [0, 1, 2, 3]);
        draft.change(Change::Core(8), &host);
        assert_eq!(draft.cpus, 5);
        draft.change(Change::Core(0), &host);
        assert_eq!(draft.cpus, 4);
        draft.change(Change::Cpus(2), &host);
        assert_eq!(draft.resources.host_cpus, [1, 2]);
        draft.change(Change::Core(1), &host);
        draft.change(Change::Core(999), &host);
        assert_eq!(draft.resources.host_cpus, [1, 2]);
        assert_eq!(draft.cpus, 2);
        draft.change(Change::Cpus(4), &host);
        assert_eq!(draft.resources.host_cpus, [1, 2, 0, 3]);
    }
    #[test]
    fn presets_use_only_reported_kinds_and_available_threads() {
        let host = host();
        let mut draft = Draft::new(Some(&host), Requirements::default());
        draft.change(Change::Cpus(10), &host);
        draft.change(Change::Placement(Some("Efficiency")), &host);
        assert_eq!(draft.resources.host_cpus, [4, 5, 6, 7]);
        assert_eq!(draft.cpus, 4);
        draft.change(Change::Placement(Some("unknown")), &host);
        assert_eq!(draft.cpus, 4);
        draft.change(Change::Placement(None), &host);
        assert_eq!(draft.resources.host_cpus, [0, 1, 2, 3]);
    }
}
