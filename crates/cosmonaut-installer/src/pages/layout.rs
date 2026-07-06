//! Custom-layout page: assign roles to existing partitions and/or
//! create the new root in a free-space gap. Follows the wifi page's
//! pattern — all state in [`LayoutUiState`], interactions namespaced
//! under [`LayoutMsg`].
//!
//! Scope (v1): create-root targets *pre-existing* gaps only. To install
//! over an existing partition, assign it "Use as root" (formatted in
//! place) instead of delete-then-create — same net effect without
//! GUI-side geometry math on freed space.

use cosmic::iced::Length;
use cosmic::widget::{self, button, column, radio, row, scrollable, settings, text, text_input};
use cosmic::Element;

use cosmonaut_engine::{PartitionAction, ESP_BYTES, ROOT_MIN_BYTES};

use cosmonaut_engine::probe::{human_size, DiskInfo, Gap};

use crate::app::Message;
use crate::pages::{wizard_frame, Page};
use crate::widgets::disk_bar::{self, BarSegment, SegRole};

/// GPT ESP type GUID (lowercase), for defaulting an existing ESP row.
const GPT_ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// Role dropdown entries, in display order.
const ROLE_LABELS: &[&str] = &[
    "Leave alone",
    "Use as root (format btrfs)",
    "Use as ESP (keep contents)",
    "Use as ESP (format)",
    "Delete",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowRole {
    #[default]
    Leave,
    Root,
    EspKeep,
    EspFormat,
    Delete,
}

impl RowRole {
    fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Root,
            2 => Self::EspKeep,
            3 => Self::EspFormat,
            4 => Self::Delete,
            _ => Self::Leave,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Leave => 0,
            Self::Root => 1,
            Self::EspKeep => 2,
            Self::EspFormat => 3,
            Self::Delete => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LayoutMsg {
    RoleSelected(usize, usize),
    /// `None` = no new partition (root comes from an existing one).
    GapSelected(Option<usize>),
    RootSizeChanged(String),
}

impl From<LayoutMsg> for Message {
    fn from(m: LayoutMsg) -> Self {
        Message::Layout(m)
    }
}

#[derive(Debug, Default)]
pub struct LayoutUiState {
    /// Snapshot of the disk this state was built for; rebuilt when the
    /// selection changes or the page is re-entered after a rescan.
    pub disk: Option<DiskInfo>,
    /// Parallel to `disk.partitions`.
    roles: Vec<RowRole>,
    /// Index into `disk.gaps` for the new root, if creating one.
    gap_idx: Option<usize>,
    /// Root size in GiB as typed; empty = fill the gap.
    root_size_gib: String,
}

impl LayoutUiState {
    /// (Re)initialize for `disk`. Existing ESP defaults to "keep".
    pub fn reset_for(&mut self, disk: &DiskInfo) {
        self.roles = disk
            .partitions
            .iter()
            .map(|p| {
                if p.part_type.as_deref() == Some(GPT_ESP) {
                    RowRole::EspKeep
                } else {
                    RowRole::Leave
                }
            })
            .collect();
        // Default to creating root in the largest gap when one can fit it.
        self.gap_idx = disk
            .gaps
            .iter()
            .enumerate()
            .max_by_key(|(_, g)| g.size_bytes)
            .filter(|(_, g)| g.size_bytes >= ROOT_MIN_BYTES)
            .map(|(i, _)| i);
        self.root_size_gib.clear();
        self.disk = Some(disk.clone());
    }

    pub fn update(&mut self, msg: LayoutMsg) {
        match msg {
            LayoutMsg::RoleSelected(row, role_idx) => {
                let role = RowRole::from_index(role_idx);
                if let Some(r) = self.roles.get_mut(row) {
                    *r = role;
                }
                // Single-root / single-ESP invariants: picking a role
                // clears the same role elsewhere.
                let clear_others = |roles: &mut Vec<RowRole>, of: &[RowRole]| {
                    for (i, r) in roles.iter_mut().enumerate() {
                        if i != row && of.contains(r) {
                            *r = RowRole::Leave;
                        }
                    }
                };
                match role {
                    RowRole::Root => {
                        clear_others(&mut self.roles, &[RowRole::Root]);
                        self.gap_idx = None;
                    }
                    RowRole::EspKeep | RowRole::EspFormat => {
                        clear_others(&mut self.roles, &[RowRole::EspKeep, RowRole::EspFormat]);
                    }
                    _ => {}
                }
            }
            LayoutMsg::GapSelected(idx) => {
                self.gap_idx = idx;
                if idx.is_some() {
                    for r in &mut self.roles {
                        if *r == RowRole::Root {
                            *r = RowRole::Leave;
                        }
                    }
                }
            }
            LayoutMsg::RootSizeChanged(s) => {
                if s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 6 {
                    self.root_size_gib = s;
                }
            }
        }
    }

    fn esp_row(&self) -> Option<(usize, RowRole)> {
        self.roles
            .iter()
            .enumerate()
            .find(|(_, r)| matches!(r, RowRole::EspKeep | RowRole::EspFormat))
            .map(|(i, r)| (i, *r))
    }

    fn root_row(&self) -> Option<usize> {
        self.roles.iter().position(|r| *r == RowRole::Root)
    }

    /// Requested root size in bytes, if creating in a gap.
    fn requested_root_bytes(&self, gap: &Gap, esp_carved: bool) -> u64 {
        let capacity = gap.size_bytes - if esp_carved { ESP_BYTES } else { 0 };
        match self.root_size_gib.parse::<u64>() {
            Ok(gib) if gib > 0 => (gib * 1024 * 1024 * 1024).min(capacity),
            _ => capacity,
        }
    }

    /// Validate; Ok(summary line) or Err(problem line).
    pub fn validate(&self) -> Result<String, String> {
        let disk = self.disk.as_ref().ok_or("no disk selected")?;
        let esp = self.esp_row();
        let root_from_row = self.root_row();
        let root_from_gap = self.gap_idx;

        match (root_from_row, root_from_gap) {
            (None, None) => {
                return Err(
                    "Choose a root: assign \"Use as root\" to a partition or pick a free-space gap"
                        .into(),
                )
            }
            (Some(_), Some(_)) => return Err("Only one root allowed".into()),
            _ => {}
        }

        if let Some(i) = root_from_row {
            let p = &disk.partitions[i];
            if p.mounted {
                return Err(format!("{} is mounted", p.path.display()));
            }
            if p.size_bytes < ROOT_MIN_BYTES {
                return Err(format!(
                    "{} is too small for root (minimum {})",
                    p.path.display(),
                    human_size(ROOT_MIN_BYTES)
                ));
            }
        }

        // ESP: an assigned row, or auto-created in the gap.
        let esp_auto = esp.is_none();
        if esp_auto && root_from_gap.is_none() {
            return Err(
                "No ESP: assign \"Use as ESP\" to a partition (or create root in a gap so one \
                 can be created alongside it)"
                    .into(),
            );
        }

        if let Some(gi) = root_from_gap {
            let gap = disk.gaps.get(gi).ok_or("selected gap no longer exists")?;
            let need = ROOT_MIN_BYTES + if esp_auto { ESP_BYTES } else { 0 };
            if gap.size_bytes < need {
                return Err(format!(
                    "Selected gap is too small ({}; need at least {})",
                    human_size(gap.size_bytes),
                    human_size(need)
                ));
            }
            let root = self.requested_root_bytes(gap, esp_auto);
            if root < ROOT_MIN_BYTES {
                return Err(format!(
                    "Root size too small (minimum {})",
                    human_size(ROOT_MIN_BYTES)
                ));
            }
        }

        for (i, r) in self.roles.iter().enumerate() {
            if *r != RowRole::Leave && disk.partitions[i].mounted {
                return Err(format!(
                    "{} is mounted and can't be modified",
                    disk.partitions[i].path.display()
                ));
            }
        }

        Ok(self.summary())
    }

    fn summary(&self) -> String {
        let Some(disk) = &self.disk else {
            return String::new();
        };
        let mut parts = Vec::new();
        if let Some(i) = self.root_row() {
            parts.push(format!(
                "root on {} (formatted)",
                disk.partitions[i].path.display()
            ));
        }
        if let Some(gi) = self.gap_idx {
            if let Some(gap) = disk.gaps.get(gi) {
                let esp_auto = self.esp_row().is_none();
                let root = self.requested_root_bytes(gap, esp_auto);
                parts.push(format!("new {} root in free space", human_size(root)));
                if esp_auto {
                    parts.push("new 512 MiB ESP".into());
                }
            }
        }
        if let Some((i, role)) = self.esp_row() {
            parts.push(format!(
                "ESP on {}{}",
                disk.partitions[i].path.display(),
                if role == RowRole::EspFormat {
                    " (formatted)"
                } else {
                    " (kept)"
                }
            ));
        }
        let deletes = self.roles.iter().filter(|r| **r == RowRole::Delete).count();
        if deletes > 0 {
            parts.push(format!("{deletes} partition(s) deleted"));
        }
        parts.join(", ")
    }

    /// Convert to engine actions. Call only when `validate()` is Ok.
    pub fn to_actions(&self) -> Vec<PartitionAction> {
        let Some(disk) = &self.disk else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        for (i, role) in self.roles.iter().enumerate() {
            let device = disk.partitions[i].path.clone();
            match role {
                RowRole::Leave => {}
                RowRole::Root => actions.push(PartitionAction::UseAsRoot { device }),
                RowRole::EspKeep => actions.push(PartitionAction::UseAsEsp {
                    device,
                    format: false,
                }),
                RowRole::EspFormat => actions.push(PartitionAction::UseAsEsp {
                    device,
                    format: true,
                }),
                RowRole::Delete => actions.push(PartitionAction::Delete { device }),
            }
        }
        if let Some(gi) = self.gap_idx {
            if let Some(gap) = disk.gaps.get(gi) {
                let esp_auto = self.esp_row().is_none();
                let mut start = gap.start_bytes;
                if esp_auto {
                    actions.push(PartitionAction::CreateEsp { start_bytes: start });
                    start += ESP_BYTES;
                }
                let root = self.requested_root_bytes(gap, esp_auto);
                actions.push(PartitionAction::CreateRoot {
                    start_bytes: start,
                    size_bytes: Some(root),
                });
            }
        }
        actions
    }

    /// Segments for the "after" preview bar.
    pub fn planned_segments(&self) -> Vec<BarSegment> {
        let Some(disk) = &self.disk else {
            return Vec::new();
        };
        #[derive(Clone)]
        struct Item {
            start: u64,
            seg: BarSegment,
        }
        let mut items: Vec<Item> = Vec::new();
        for (i, p) in disk.partitions.iter().enumerate() {
            let role = self.roles.get(i).copied().unwrap_or_default();
            let name = p
                .label
                .clone()
                .or_else(|| p.fstype.clone())
                .unwrap_or_else(|| format!("partition {}", p.number));
            let seg = match role {
                RowRole::Delete => BarSegment {
                    label: "free space".into(),
                    size_bytes: p.size_bytes,
                    role: SegRole::Free,
                },
                RowRole::Root => BarSegment {
                    label: "root".into(),
                    size_bytes: p.size_bytes,
                    role: SegRole::Root,
                },
                RowRole::EspKeep | RowRole::EspFormat => BarSegment {
                    label: "EFI".into(),
                    size_bytes: p.size_bytes,
                    role: SegRole::Esp,
                },
                RowRole::Leave => BarSegment {
                    label: name,
                    size_bytes: p.size_bytes,
                    role: SegRole::Existing,
                },
            };
            items.push(Item {
                start: p.start_bytes,
                seg,
            });
        }
        for (gi, gap) in disk.gaps.iter().enumerate() {
            if Some(gi) == self.gap_idx {
                let esp_auto = self.esp_row().is_none();
                let mut start = gap.start_bytes;
                if esp_auto {
                    items.push(Item {
                        start,
                        seg: BarSegment {
                            label: "EFI".into(),
                            size_bytes: ESP_BYTES,
                            role: SegRole::Esp,
                        },
                    });
                    start += ESP_BYTES;
                }
                let root = self.requested_root_bytes(gap, esp_auto);
                items.push(Item {
                    start,
                    seg: BarSegment {
                        label: "root".into(),
                        size_bytes: root,
                        role: SegRole::Root,
                    },
                });
                let used = root + if esp_auto { ESP_BYTES } else { 0 };
                if gap.size_bytes > used {
                    items.push(Item {
                        start: start + root,
                        seg: BarSegment {
                            label: "free space".into(),
                            size_bytes: gap.size_bytes - used,
                            role: SegRole::Free,
                        },
                    });
                }
            } else {
                items.push(Item {
                    start: gap.start_bytes,
                    seg: BarSegment {
                        label: "free space".into(),
                        size_bytes: gap.size_bytes,
                        role: SegRole::Free,
                    },
                });
            }
        }
        items.sort_by_key(|i| i.start);
        items.into_iter().map(|i| i.seg).collect()
    }
}

pub fn view(state: &LayoutUiState) -> Element<'_, Message> {
    let Some(disk) = &state.disk else {
        return wizard_frame(
            "Custom layout",
            Some("No disk selected."),
            widget::column::with_capacity(0).into(),
            row::with_capacity(1)
                .push(button::standard("Back").on_press(Message::Back))
                .into(),
            Page::CustomLayout,
        );
    };

    let mut body = column::with_capacity(5).spacing(20);

    // After-preview bar.
    let planned = state.planned_segments();
    body = body.push(disk_bar::view("Planned layout", &planned, disk.size_bytes));

    // Partition rows with role dropdowns.
    if !disk.partitions.is_empty() {
        let mut section = settings::section().title("Partitions");
        for (i, p) in disk.partitions.iter().enumerate() {
            let name = format!(
                "{}  {}  {}{}",
                p.path.display(),
                human_size(p.size_bytes),
                p.fstype.as_deref().unwrap_or("—"),
                if p.mounted { "  (mounted)" } else { "" },
            );
            let selected = state.roles.get(i).copied().unwrap_or_default().index();
            section = section.add(settings::item::builder(name).control(widget::dropdown(
                ROLE_LABELS,
                Some(selected),
                move |idx| Message::Layout(LayoutMsg::RoleSelected(i, idx)),
            )));
        }
        body = body.push(section);
    }

    // Gap selection for the new root.
    let mut gap_section = settings::section().title("New root partition");
    gap_section = gap_section.add(radio(
        text::body("Don't create one (use a partition above as root)"),
        None::<usize>,
        Some(state.gap_idx),
        |v| Message::Layout(LayoutMsg::GapSelected(v)),
    ));
    for (gi, gap) in disk.gaps.iter().enumerate() {
        gap_section = gap_section.add(radio(
            text::body(format!(
                "Create in free space at {} ({} available)",
                human_size(gap.start_bytes),
                human_size(gap.size_bytes)
            )),
            Some(gi),
            Some(state.gap_idx),
            |v| Message::Layout(LayoutMsg::GapSelected(v)),
        ));
    }
    if state.gap_idx.is_some() {
        gap_section = gap_section.add(
            settings::item::builder("Root size (GiB, empty = use all)").control(
                text_input("all", &state.root_size_gib)
                    .on_input(|s| Message::Layout(LayoutMsg::RootSizeChanged(s))),
            ),
        );
    }
    body = body.push(gap_section);

    // Validation line.
    let validation = state.validate();
    let (caption, valid) = match &validation {
        Ok(summary) => (format!("Plan: {summary}"), true),
        Err(problem) => (problem.clone(), false),
    };
    body = body.push(text::caption(caption));

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::suggested("Continue").on_press_maybe(valid.then_some(Message::Next)));

    wizard_frame(
        "Custom layout",
        Some(
            "Assign roles to existing partitions, or create the new system in free space. \
             Partitions marked \"Leave alone\" are not touched.",
        ),
        scrollable(body)
            .height(Length::Fill)
            .width(Length::Fill)
            .into(),
        nav.into(),
        Page::CustomLayout,
    )
}
