//! Hardware form. Admission and defaults live in vkored; this module only
//! edits a draft and presents the current host snapshot.
use crate::{Message, UiElement, machines::MachineMessage, theme, widgets};
use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, slider};
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
                self.resources.host_cpus = host.preferred_cpus(n);
            }
            Change::Memory(n) => self.memory_mib = n,
            Change::Disk(n) if !self.storage_fixed => self.disk_mib = u64::from(n),
            Change::PinCpus if host.cpu_pinning => {
                self.resources.pin_cpus = !self.resources.pin_cpus;
                if self.resources.pin_cpus {
                    self.resources.host_cpus = host.preferred_cpus(self.cpus);
                }
            }
            Change::PinMemory if host.memory_pinning => {
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
            Change::Core(id) if self.resources.pin_cpus => {
                if let Some(index) = self.resources.host_cpus.iter().position(|c| *c == id) {
                    self.resources.host_cpus.remove(index);
                } else if self.resources.host_cpus.len() < 128 {
                    self.resources.host_cpus.push(id);
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
fn action(label: &str, message: Option<Message>, selected: bool) -> UiElement<'static> {
    button(widgets::body(label.to_owned(), 13., theme::TEXT))
        .padding([7, 10])
        .style(widgets::button_style(widgets::Button::Choice(selected)))
        .on_press_maybe(message)
        .into()
}
fn switch(label: &str, enabled: bool, available: bool, message: Message) -> UiElement<'static> {
    action(
        &format!("{}  {label}", if enabled { "[x]" } else { "[ ]" }),
        available.then_some(message),
        enabled,
    )
}
pub fn view<'a>(
    draft: &'a Draft,
    host: &'a Result<HostResources, String>,
    editing: bool,
    busy: bool,
) -> UiElement<'a> {
    let send = move |c| Message::Machine(MachineMessage::Hardware(editing, c));
    let Ok(host) = host else {
        return widgets::data(host.as_ref().unwrap_err(), theme::ERROR).into();
    };
    let max_cpu = (host.logical_cpus.min(128) as u16).max(draft.minimum.cpus);
    let max_ram = (host.memory_mib.min(1 << 20) as u32).max(draft.minimum.memory_mib);
    let ram_step = (draft.resources.pages.mib() * 2).max(2);
    let max_disk = host
        .storage_available_mib
        .clamp(draft.minimum.disk_mib, 1 << 22) as u32;
    let cpu_slider = slider(draft.minimum.cpus..=max_cpu, draft.cpus, move |n| {
        send(Change::Cpus(n))
    })
    .step(1u16)
    .style(widgets::resource_slider);
    let ram_slider = slider(
        draft.minimum.memory_mib..=max_ram,
        draft.memory_mib,
        move |n| send(Change::Memory(n / ram_step * ram_step)),
    )
    .step(ram_step)
    .style(widgets::resource_slider);
    let mut content = column![
        widgets::body("Hardware", 18., theme::TEXT),
        widgets::data(&host.cpu_name, theme::TEXT_SECONDARY),
        widgets::data(
            format!(
                "{} physical cores · {} logical CPUs",
                host.physical_cores, host.logical_cpus
            ),
            theme::TEXT_MUTED
        ),
        widgets::data(&host.core_summary, theme::TEXT_MUTED),
        action(
            "Use recommended settings",
            (!busy).then(|| send(Change::Defaults)),
            false
        ),
        widgets::label(format!("{} guest CPUs", draft.cpus), theme::TEXT),
        cpu_slider,
        widgets::data(
            format!("Minimum {} · maximum {}", draft.minimum.cpus, max_cpu),
            theme::TEXT_MUTED
        ),
        switch(
            "Pin guest CPUs",
            draft.resources.pin_cpus,
            host.cpu_pinning && !busy,
            send(Change::PinCpus)
        ),
    ]
    .spacing(12);
    if host.cpu_pinning {
        content=content.push(widgets::body("Each guest CPU stays on one selected host thread. Other host tasks may still use that core.",12.,theme::TEXT_MUTED));
        if draft.resources.pin_cpus {
            content = content.push(action(
                if draft.show_cores {
                    "Hide core selection"
                } else {
                    "Choose exact cores"
                },
                Some(send(Change::ShowCores)),
                draft.show_cores,
            ));
            content = content.push(widgets::data(
                format!(
                    "{} of {} threads selected",
                    draft.resources.host_cpus.len(),
                    draft.cpus
                ),
                if draft.resources.host_cpus.len() == usize::from(draft.cpus) {
                    theme::TEXT_MUTED
                } else {
                    theme::WARNING
                },
            ));
            if draft.show_cores {
                let mut cores = column![].spacing(8);
                for core in &host.cores {
                    let mut buttons = row![].spacing(6);
                    for &id in &core.threads {
                        buttons = buttons.push(action(
                            &format!("CPU {id}"),
                            (!busy).then(|| send(Change::Core(id))),
                            draft.resources.host_cpus.contains(&id),
                        ));
                    }
                    cores = cores.push(
                        column![
                            widgets::data(
                                format!(
                                    "{} · socket {} · core {}",
                                    core.kind, core.package, core.id
                                ),
                                theme::TEXT_MUTED
                            ),
                            buttons.wrap()
                        ]
                        .spacing(5),
                    );
                }
                content = content.push(widgets::scroll(cores).height(220));
            }
        }
    } else {
        content = content.push(widgets::body(
            "macOS manages core placement; exact CPU pinning is unavailable.",
            12.,
            theme::TEXT_MUTED,
        ));
    }
    content = content
        .push(widgets::label(
            format!("{:.2} GiB memory", f64::from(draft.memory_mib) / 1024.),
            theme::TEXT,
        ))
        .push(ram_slider)
        .push(widgets::data(
            format!(
                "{:.1} GiB physical · {:.1} GiB available",
                host.memory_mib as f64 / 1024.,
                host.available_memory_mib as f64 / 1024.
            ),
            theme::TEXT_MUTED,
        ))
        .push(switch(
            "Pin memory in RAM",
            draft.resources.pin_memory,
            host.memory_pinning && !busy,
            send(Change::PinMemory),
        ));
    if draft.resources.pages == PageSize::System
        && u64::from(draft.memory_mib) > host.available_memory_mib
    {
        content=content.push(widgets::body(if draft.resources.pin_memory {
            "Not enough available RAM to pin this allocation. Reduce it or close other workloads."
        } else { "This exceeds currently available RAM. The host may swap; reduce the allocation or close other workloads." },12.,theme::WARNING));
    }
    for node in &host.memory_nodes {
        content = content.push(widgets::data(
            format!(
                "NUMA {} · {:.1} GiB · CPUs {}",
                node.id,
                node.memory_mib as f64 / 1024.,
                node.cpus
            ),
            theme::TEXT_MUTED,
        ));
    }
    if !host.memory_pinning {
        content = content.push(widgets::body(
            "Memory locking is unavailable in QEMU on this host.",
            12.,
            theme::TEXT_MUTED,
        ));
    } else if draft.resources.pin_memory
        && host
            .memlock_bytes
            .is_some_and(|n| n < vkored::hardware::lock_budget(draft.memory_mib))
    {
        content=content.push(widgets::body("Authorization is needed to keep this much memory resident. QEMU continues to run as your user.",12.,theme::WARNING))
            .push(action("Authorize memory…",(!busy).then_some(Message::Machine(MachineMessage::AuthorizeMemory(draft.memory_mib))),false));
    }
    content = content.push(widgets::label("Physical page size", theme::TEXT_MUTED));
    for page in [PageSize::System, PageSize::Huge2M, PageSize::Huge1G] {
        content = content.push(action(
            page.label(),
            (!busy && host.huge_available(page, draft.memory_mib))
                .then(|| send(Change::Page(page))),
            draft.resources.pages == page,
        ));
    }
    content=content.push(widgets::body("2 MiB is recommended when an accessible hugepage pool has enough space. Unavailable sizes are disabled.",12.,theme::TEXT_MUTED));
    content = content.push(widgets::label(
        format!("{:.1} GiB storage", draft.disk_mib as f64 / 1024.),
        theme::TEXT,
    ));
    if draft.storage_fixed {
        content = content.push(widgets::body(
            "Disk size and reservation are fixed after first launch.",
            12.,
            theme::TEXT_MUTED,
        ));
    } else {
        content = content
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
                "Reserve disk space",
                draft.resources.reserve_storage,
                !busy,
                send(Change::Reserve),
            ));
    }
    content=content.push(widgets::data(format!("{:.1} GiB free on the storage filesystem",host.storage_available_mib as f64/1024.),theme::TEXT_MUTED))
        .push(widgets::body("Reserves space at first launch and asks the filesystem for contiguous placement where supported.",12.,theme::TEXT_MUTED))
        .push(action("Storage details",Some(send(Change::ShowStorage)),draft.show_storage));
    if draft.show_storage {
        content = content.push(widgets::data(
            host.storage_path.display().to_string(),
            theme::TEXT_SECONDARY,
        ));
        for disk in &host.storage {
            content = content.push(widgets::data(
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
    content = content.push(action(
        "Refresh host resources",
        (!busy).then_some(Message::Machine(MachineMessage::RefreshHardware)),
        false,
    ));
    container(content.align_x(alignment::Horizontal::Left))
        .width(Length::Fill)
        .into()
}
