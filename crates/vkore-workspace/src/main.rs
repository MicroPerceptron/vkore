use std::{env, path::Path};

use kore_workspace_sync::ChangeKind;
use workspacectl::{
    Artifact, DEFAULT_IMAGE_BYTES, ExportPlan, parse_digest, seed_image, snapshot_hex,
};

fn usage() -> ! {
    eprintln!(
        "usage: vkore-workspace snapshot <root> <output.kws> [--parent <snapshot-id>] | verify <snapshot.kws> | \
         image-import <snapshot.kws> <output.raw> [--bytes <size>] | \
         diff <base.kws> <desired.kws> | apply <root> <base.kws> <desired.kws> <result.kwr> \
         --approve <change-set-id>"
    );
    std::process::exit(2)
}

fn main() {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| usage());
    let result = match command.as_str() {
        "snapshot" => {
            let root = args.next().unwrap_or_else(|| usage());
            let output = args.next().unwrap_or_else(|| usage());
            let parent = match args.next() {
                None => [0; 32],
                Some(flag) if flag == "--parent" => args
                    .next()
                    .and_then(|value| parse_digest(&value))
                    .unwrap_or_else(|| usage()),
                Some(_) => usage(),
            };
            if args.next().is_some() {
                usage();
            }
            Artifact::scan(Path::new(&root), parent).and_then(|artifact| {
                artifact.write_new(Path::new(&output))?;
                println!(
                    "snapshot {}",
                    snapshot_hex(&artifact.manifest().snapshot_id())
                );
                Ok(())
            })
        }
        "verify" => {
            let input = args.next().unwrap_or_else(|| usage());
            if args.next().is_some() {
                usage();
            }
            Artifact::read(Path::new(&input)).map(|artifact| {
                println!(
                    "snapshot {} entries={} bytes={}",
                    snapshot_hex(&artifact.manifest().snapshot_id()),
                    artifact.manifest().entries().len(),
                    artifact.manifest().header().total_bytes.get()
                );
            })
        }
        "image-import" => {
            let input = args.next().unwrap_or_else(|| usage());
            let output = args.next().unwrap_or_else(|| usage());
            let image_bytes = match args.next() {
                None => DEFAULT_IMAGE_BYTES,
                Some(flag) if flag == "--bytes" => args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage()),
                Some(_) => usage(),
            };
            if args.next().is_some() {
                usage();
            }
            Artifact::read(Path::new(&input)).and_then(|mut artifact| {
                let snapshot = seed_image(&mut artifact, Path::new(&output), image_bytes)?;
                println!(
                    "image {} bytes={} snapshot={}",
                    output,
                    image_bytes,
                    snapshot_hex(&snapshot)
                );
                Ok(())
            })
        }
        "diff" => {
            let base = args.next().unwrap_or_else(|| usage());
            let desired = args.next().unwrap_or_else(|| usage());
            if args.next().is_some() {
                usage();
            }
            Artifact::read(Path::new(&base)).and_then(|base| {
                let desired = Artifact::read(Path::new(&desired))?;
                let plan = ExportPlan::between(&base, &desired)?;
                println!(
                    "change-set {} operations={}",
                    snapshot_hex(&plan.id()),
                    plan.changes().len()
                );
                for change in plan.changes() {
                    let kind = match change.kind {
                        ChangeKind::CreateDirectory => "mkdir",
                        ChangeKind::CreateFile => "create",
                        ChangeKind::ModifyFile => "modify",
                        ChangeKind::Delete => "delete",
                        ChangeKind::Rename => "rename",
                    };
                    println!(
                        "{kind}\t{}\t{}\texpect={}\treplace={}\tbytes={}\texecutable={}",
                        change.path.as_str(),
                        change.target.as_ref().map_or("-", |path| path.as_str()),
                        snapshot_hex(&change.expected),
                        snapshot_hex(&change.replacement),
                        change.replacement_len,
                        change.executable,
                    );
                }
                Ok(())
            })
        }
        "apply" => {
            let root = args.next().unwrap_or_else(|| usage());
            let base = args.next().unwrap_or_else(|| usage());
            let desired = args.next().unwrap_or_else(|| usage());
            let result = args.next().unwrap_or_else(|| usage());
            if args.next().as_deref() != Some("--approve") {
                usage();
            }
            let approval = args.next().and_then(|value| parse_digest(&value));
            if args.next().is_some() || approval.is_none() {
                usage();
            }
            let approval = approval.unwrap_or([0; 32]);
            Artifact::read(Path::new(&base)).and_then(|base| {
                let desired = Artifact::read(Path::new(&desired))?;
                let plan = ExportPlan::between(&base, &desired)?;
                let outcome = plan.apply(
                    Path::new(&root),
                    &base,
                    &desired,
                    approval,
                    Path::new(&result),
                )?;
                println!(
                    "export {} applied={} snapshot={}",
                    if outcome.complete {
                        "complete"
                    } else {
                        "partial"
                    },
                    outcome.applied,
                    snapshot_hex(&outcome.snapshot)
                );
                if outcome.complete {
                    Ok(())
                } else {
                    Err(workspacectl::Error::Partial {
                        applied: usize::from(outcome.applied),
                    })
                }
            })
        }
        _ => usage(),
    };
    if let Err(error) = result {
        eprintln!("vkore-workspace: {error}");
        std::process::exit(1);
    }
}
