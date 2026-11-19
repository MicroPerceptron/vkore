//! Hardware form. Admission and defaults live in vkored; this module only
//! edits a draft and presents the current host snapshot.
use crate::typography::Typography;
use crate::{Message, UiElement, machines::MachineMessage, theme, widgets};
use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, slider, space, toggler};
use vkore_vm::hardware::{HostResources, PageSize, Resources};
use vkored::{MachineConfig, hardware::Requirements};

#[derive(Clone, Debug)]
pub struct Draft {
    pub cpus: u16,
    pub memory_mib: u32,
    pub disk_mib: u64,
    pub resources: Resources,
    pub minimum: Requirements,
    pub storage_fixed: bool,
    pub show_cores: bool,
    pub show_storage: bool,
}
#[derive(Clone, Debug)]
pub enum Change {
    Cpus(u16),
    Memory(u32),
    Disk(u32),
    PinCpus,
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
                minimum.disk_mib,
                Resources::default(),
            ));
        Self {
            cpus,
            memory_mib,
            disk_mib,
            resources,
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
            disk_mib: c.disk_mib,
            resources: c.resources.clone(),
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
    pub fn change(&mut self, change: Change, host: &HostResources) {
        match change {
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
            Change::Memory(n) => self.memory_mib = n,
            Change::Disk(n) if !self.storage_fixed => self.disk_mib = u64::from(n),
            Change::PinCpus if host.cpu_pinning => {
                self.resources.pin_cpus = !self.resources.pin_cpus;
                if self.resources.pin_cpus {
                    self.resources.host_cpus = host.preferred_cpus(self.cpus);
                }
            }
            Change::PinMemory if host.memory_pinning || self.resources.pin_memory => {
                self.resources.pin_memory = !self.resources.pin_memory
            }
            Change::Reserve if !self.storage_fixed => {
                self.resources.reserve_storage = !self.resources.reserve_storage
            }
            Change::Page(page) if host.huge_available(page, self.memory_mib) => {
                self.resources.pages = page;
                self.memory_mib =
                    self.memory_mib.div_ceil((page.mib() * 2).max(2)) * (page.mib() * 2).max(2);
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
                if self.storage_fixed {
                    fresh.disk_mib = self.disk_mib;
                    fresh.resources.reserve_storage = self.resources.reserve_storage;
                    fresh.storage_fixed = true;
                }
                *self = fresh;
            }
            _ => {}
        }
    }
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
fn action(
    ty: Typography,
    label: &str,
    message: Option<Message>,
    selected: bool,
) -> UiElement<'static> {
    button(ty.body(label.to_owned(), 13., theme::TEXT))
        .padding([7, 10])
        .style(widgets::button_style(widgets::Button::Choice(selected)))
        .on_press_maybe(message)
        .into()
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
fn section<'a>(content: impl Into<UiElement<'a>>) -> UiElement<'a> {
    container(content)
        .padding(16)
        .width(Length::Fill)
        .style(|appearance| {
            theme::panel(appearance, theme::PANEL_FILE, Some(theme::SEPARATOR), 10.)
        })
        .into()
}
fn heading(ty: Typography, title: &'static str, value: String) -> UiElement<'static> {
    row![
        ty.body(title, 18., theme::TEXT),
        space::horizontal(),
        ty.badge(value, theme::TEXT),
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
    let max_ram = (host.memory_mib.min(1 << 20) as u32).max(draft.minimum.memory_mib);
    let ram_step = (draft.resources.pages.mib() * 2).max(2);
    let max_disk = host
        .storage_available_mib
        .clamp(draft.minimum.disk_mib, 1 << 22) as u32;
    let mut cpu = column![
        heading(ty, "Processors", format!("{} guest CPUs", draft.cpus)),
        ty.data(&host.cpu_name, theme::TEXT_SECONDARY),
        ty.data(
            format!(
                "{} physical cores · {} logical CPUs",
                host.physical_cores, host.logical_cpus
            ),
            theme::TEXT_MUTED
        ),
    ]
    .spacing(12);
    if !host.core_summary.is_empty() {
        cpu = cpu.push(ty.data(&host.core_summary, theme::TEXT_MUTED));
    }
    cpu = cpu
        .push(
            row![
                action(
                    ty,
                    "−",
                    (!busy && draft.cpus > draft.minimum.cpus)
                        .then(|| send(Change::Cpus(draft.cpus.saturating_sub(1)))),
                    false
                ),
                slider(draft.minimum.cpus..=max_cpu, draft.cpus, move |n| send(
                    Change::Cpus(n)
                ))
                .step(1u16)
                .style(widgets::resource_slider),
                action(
                    ty,
                    "+",
                    (!busy && draft.cpus < max_cpu)
                        .then(|| send(Change::Cpus(draft.cpus.saturating_add(1)))),
                    false
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
    if host.cpu_pinning {
        cpu = cpu.push(ty.body("Keep each guest CPU on a chosen host thread. Pinning does not reserve the core exclusively.", 12., theme::TEXT_MUTED));
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
                    .push(ty.body(format!("Select threads to adjust the CPU count. Keep at least {} selected. Siblings share one physical core.", draft.minimum.cpus), 12., theme::TEXT_MUTED));
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
                                theme::panel(
                                    appearance,
                                    theme::PANEL,
                                    Some(if selected > 0 {
                                        theme::BORDER
                                    } else {
                                        theme::SEPARATOR
                                    }),
                                    8.,
                                )
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
                cpu = cpu.push(ty.body("This saved selection no longer matches the CPU count. Choose a placement preset or adjust the count.", 12., theme::WARNING));
            }
        }
    } else {
        cpu = cpu.push(ty.body("macOS manages core placement. You can adjust the CPU count; exact pinning is unavailable.", 12., theme::TEXT_MUTED));
    }
    let mut memory = column![
        heading(
            ty,
            "Memory",
            format!("{:.2} GiB", f64::from(draft.memory_mib) / 1024.)
        ),
        slider(
            draft.minimum.memory_mib..=max_ram,
            draft.memory_mib,
            move |n| send(Change::Memory(n / ram_step * ram_step))
        )
        .step(ram_step)
        .style(widgets::resource_slider),
        ty.data(
            format!(
                "{:.1} GiB physical · {:.1} GiB free",
                host.memory_mib as f64 / 1024.,
                host.free_memory_mib as f64 / 1024.
            ),
            theme::TEXT_MUTED
        ),
        ty.data(
            format!(
                "{:.1} GiB free or reclaimable (OS estimate)",
                host.available_memory_mib as f64 / 1024.
            ),
            theme::TEXT_MUTED
        ),
        switch(
            ty,
            "Pin memory in RAM",
            draft.resources.pin_memory,
            (host.memory_pinning || draft.resources.pin_memory) && !busy,
            send(Change::PinMemory)
        ),
    ]
    .spacing(12);
    if draft.resources.pages == PageSize::System
        && u64::from(draft.memory_mib) > host.available_memory_mib
    {
        memory = memory.push(ty.body(if draft.resources.pin_memory {
            "Not enough available RAM to pin this allocation. Reduce it or close other workloads."
        } else { "This exceeds currently available RAM. The host may swap; reduce the allocation or close other workloads." }, 12., theme::WARNING));
    }
    if !host.memory_pinning {
        memory = memory.push(ty.body(
            "Memory locking is unavailable in QEMU on this host.",
            12.,
            theme::TEXT_MUTED,
        ));
    } else if draft.resources.pin_memory
        && host
            .memlock_bytes
            .is_some_and(|n| n < vkored::hardware::lock_budget(draft.memory_mib))
    {
        memory = memory.push(ty.body(
            "Permissions are checked when you finish. System authorization appears only if needed.",
            12.,
            theme::TEXT_MUTED,
        ));
    }
    memory = memory.push(ty.label("Host memory pages", theme::TEXT_MUTED));
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
                (!busy && host.huge_available(page, draft.memory_mib))
                    .then(|| send(Change::Page(page))),
                draft.resources.pages == page,
            ));
        }
        memory = memory.push(pages.wrap());
        for page in [PageSize::Huge2M, PageSize::Huge1G] {
            if !host.huge_available(page, draft.memory_mib) {
                let reason = match host.hugepages.iter().find(|pool| pool.page == page) {
                    Some(pool) => format!(
                        "{}: {:.1} GiB available in the pool; {:.1} GiB needed.",
                        page.label(),
                        pool.free_mib as f64 / 1024.,
                        f64::from(draft.memory_mib) / 1024.
                    ),
                    None => format!(
                        "{}: no accessible reserved pool on this host.",
                        page.label()
                    ),
                };
                memory = memory.push(ty.body(reason, 12., theme::TEXT_MUTED));
            }
        }
    } else {
        memory = memory.push(pages.wrap()).push(ty.body(
            "Automatic uses this host's native pages. QEMU's explicit 2 MiB and 1 GiB hugepage backing requires Linux; it is unavailable on this host.",
            12., theme::TEXT_MUTED,
        ));
        if draft.resources.pages != PageSize::System {
            memory = memory.push(ty.body(
                format!(
                    "Saved choice: {}. Choose Automatic to use this machine on this host.",
                    draft.resources.pages.label()
                ),
                12.,
                theme::WARNING,
            ));
        }
    }
    memory = memory.push(ty.body(
        "This controls host memory backing, not page sizes inside Kore.",
        12.,
        theme::TEXT_MUTED,
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
    let mut storage = column![heading(
        ty,
        "Storage",
        format!("{:.1} GiB", draft.disk_mib as f64 / 1024.)
    )]
    .spacing(12);
    if draft.storage_fixed {
        storage = storage.push(ty.body(
            "Disk size and reservation are fixed after first launch.",
            12.,
            theme::TEXT_MUTED,
        ));
    } else {
        storage = storage
            .push(
                slider(
                    draft.minimum.disk_mib as u32..=max_disk,
                    draft.disk_mib as u32,
                    move |n| send(Change::Disk(n)),
                )
                .step(1024u32)
                .style(widgets::resource_slider),
            )
            .push(switch(
                ty,
                "Reserve disk space",
                draft.resources.reserve_storage,
                !busy,
                send(Change::Reserve),
            ));
    }
    storage = storage
        .push(ty.data(
            format!(
                "{:.1} GiB free on the storage filesystem",
                host.storage_available_mib as f64 / 1024.
            ),
            theme::TEXT_MUTED,
        ))
        .push(ty.body(
            "Reserves space at first launch and requests contiguous placement where supported.",
            12.,
            theme::TEXT_MUTED,
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
    container(
        column![
            row![
                action(
                    ty,
                    "Use recommended settings",
                    (!busy).then(|| send(Change::Defaults)),
                    false
                ),
                action(
                    ty,
                    "Refresh host",
                    (!busy).then_some(Message::Machine(MachineMessage::RefreshHardware)),
                    false
                )
            ]
            .spacing(8)
            .wrap(),
            section(cpu),
            section(memory),
            section(npu_inventory(&host.npus, ty)),
            section(storage),
        ]
        .spacing(16),
    )
    .padding(iced_core::Padding::default().right(14))
    .width(Length::Fill)
    .into()
}

pub(crate) fn npu_inventory<'a>(
    inventory: &'a vkore_vm::devices::Inventory,
    ty: Typography,
) -> iced_widget::Column<'a, Message, iced_core::Theme, crate::AxUi> {
    use vkore_vm::devices::DiscoveryStatus;
    let mut content = column![
        row![
            ty.body("Neural processors", 18., theme::TEXT),
            space::horizontal(),
            ty.badge(
                match inventory.status {
                    DiscoveryStatus::Complete => format!("{} detected", inventory.devices.len()),
                    DiscoveryStatus::Partial =>
                        format!("{} reported · incomplete", inventory.devices.len()),
                    DiscoveryStatus::Unavailable => "Discovery unavailable".into(),
                },
                theme::TEXT_MUTED
            )
        ]
        .align_y(alignment::Vertical::Center),
        ty.body(
            "Devices reported by your host. Guest accelerator connections are not available yet.",
            12.,
            theme::TEXT_MUTED
        ),
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
        content = content.push(ty.data(warning, theme::WARNING));
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
                    row![
                        ty.body(route.name, 13., theme::TEXT_SECONDARY),
                        ty.badge(route.status.label(), theme::TEXT_MUTED)
                    ]
                    .spacing(8)
                    .wrap(),
                    ty.body(route.detail, 12., theme::TEXT_MUTED),
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
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> HostResources {
        let root = tempfile::tempdir().unwrap();
        let mut host = HostResources::probe(root.path()).unwrap();
        host.cpu_pinning = true;
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
