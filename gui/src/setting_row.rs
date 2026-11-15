//! One setting as a preferences row: a slider for ranges, a dropdown for named values.
//! The window builds a row per setting, feeds it readings with [`SettingRow::show`] and
//! write results with [`SettingRow::written`]; the row calls back when the user changes it.

use adw::prelude::*;
use aocctl::ddc::Reading;
use aocctl::models::{Domain, Setting};
use aocctl::monitor::WriteStatus;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How long a slider must rest before its value is written.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The row's state from the last reading, before a fixed game mode overrides it.
#[derive(Default)]
struct BaseState {
    subtitle: String,
    sensitive: bool,
}

/// Called with a value the user picked; the window sends it to the worker and returns the
/// request's id, which comes back with its result.
pub type OnChange = Rc<dyn Fn(&'static Setting, u16) -> u64>;

pub struct SettingRow {
    pub setting: &'static Setting,
    pub row: adw::ActionRow,
    scale: Option<gtk::Scale>,
    combo: Option<adw::ComboRow>,
    status: gtk::Image,
    spinner: gtk::Spinner,
    /// Set while the code (not the user) moves the control, so that doesn't write.
    suppress: Cell<bool>,
    /// The request id and value last sent, until its result comes back.
    wanted: Cell<Option<(u64, u16)>>,
    /// The value the result icon is about; a reading of another value hides the icon.
    status_for: Cell<Option<u16>>,
    /// FPS, RTS or Racing while the monitor is in one of those fixed game modes.
    game_mode: Cell<Option<&'static str>>,
    /// The last reading's subtitle and sensitivity, before the game-mode override.
    base: RefCell<BaseState>,
    debounce: RefCell<Option<glib::SourceId>>,
    on_change: OnChange,
}

impl SettingRow {
    pub fn new(setting: &'static Setting, reading: &Result<Reading, String>, on_change: OnChange) -> Rc<Self> {
        let (row, scale, combo) = match setting.domain {
            Domain::Range { min, max } => {
                let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min.into(), max.into(), 1.0);
                scale.set_digits(0);
                scale.set_round_digits(0);
                scale.set_draw_value(true);
                scale.set_value_pos(gtk::PositionType::Left);
                scale.set_width_request(260);
                scale.set_valign(gtk::Align::Center);
                scale.update_property(&[gtk::accessible::Property::Label(setting.label)]);
                let row = adw::ActionRow::new();
                row.add_suffix(&scale);
                (row, Some(scale), None)
            }
            Domain::Enum(names) => {
                let combo = adw::ComboRow::new();
                combo.set_model(Some(&gtk::StringList::new(names)));
                (combo.clone().upcast::<adw::ActionRow>(), None, Some(combo))
            }
            Domain::Values(values) => {
                let names: Vec<&str> = values.iter().map(|(_, name)| *name).collect();
                let combo = adw::ComboRow::new();
                combo.set_model(Some(&gtk::StringList::new(&names)));
                (combo.clone().upcast::<adw::ActionRow>(), None, Some(combo))
            }
        };
        row.set_title(setting.label);
        let status = gtk::Image::new();
        status.set_visible(false);
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        row.add_suffix(&status);
        row.add_suffix(&spinner);
        let this = Rc::new(SettingRow {
            setting,
            row,
            scale,
            combo,
            status,
            spinner,
            suppress: Cell::new(false),
            wanted: Cell::new(None),
            status_for: Cell::new(None),
            game_mode: Cell::new(None),
            base: RefCell::new(BaseState::default()),
            debounce: RefCell::new(None),
            on_change,
        });
        if let Some(scale) = &this.scale {
            let weak = Rc::downgrade(&this);
            scale.connect_value_changed(move |scale| {
                if let Some(row) = weak.upgrade().filter(|r| !r.suppress.get()) {
                    row.slid(scale.value().round() as u16);
                }
            });
        }
        if let Some(combo) = &this.combo {
            let weak = Rc::downgrade(&this);
            combo.connect_selected_notify(move |combo| {
                let picked = combo.selected();
                if let Some(row) = weak.upgrade().filter(|r| !r.suppress.get())
                    && picked != gtk::INVALID_LIST_POSITION
                    && let Some(value) = row.combo_value(picked)
                {
                    row.send(value);
                }
            });
        }
        this.show(reading);
        this
    }

    /// True while a change is waiting to be sent or for its result.
    pub fn pending(&self) -> bool {
        self.wanted.get().is_some() || self.debounce.borrow().is_some()
    }

    /// Shows a value read from the monitor, unless the user's own change is still pending.
    pub fn show(&self, reading: &Result<Reading, String>) {
        if self.pending() {
            return;
        }
        let r = match reading {
            Ok(r) => *r,
            Err(e) => {
                *self.base.borrow_mut() = BaseState {
                    subtitle: format!("Couldn't read it: {e}"),
                    sensitive: false,
                };
                self.apply_availability();
                return;
            }
        };
        if self.status_for.get() != Some(r.value) {
            self.status.set_visible(false);
        }
        let mut subtitle = String::new();
        if !self.setting.verified_write {
            subtitle =
                "Read-only until its write is verified on this model (the “aocctl verify” command tests it)".into();
        }
        self.suppress.set(true);
        match (self.setting.domain, &self.scale, &self.combo) {
            (Domain::Range { min, max }, Some(scale), _) => {
                scale.set_range(min.into(), max.min(r.max).max(min).into());
                scale.set_value(r.value.into());
            }
            (Domain::Enum(names), _, Some(combo)) => {
                if (r.value as usize) < names.len() {
                    combo.set_selected(r.value.into());
                } else {
                    combo.set_selected(gtk::INVALID_LIST_POSITION);
                    subtitle = format!(
                        "The monitor reports value {}, which isn't one of these (another setting may control it)",
                        r.value
                    );
                }
            }
            (Domain::Values(values), _, Some(combo)) => {
                if let Some(i) = values.iter().position(|(value, _)| *value == r.value) {
                    combo.set_selected(i as u32);
                } else {
                    combo.set_selected(gtk::INVALID_LIST_POSITION);
                    subtitle = format!(
                        "The monitor reports value {}, which isn't one of these (another setting may control it)",
                        r.value
                    );
                }
            }
            _ => unreachable!("rows are built from their setting's domain"),
        }
        self.suppress.set(false);
        *self.base.borrow_mut() = BaseState {
            subtitle,
            sensitive: self.setting.verified_write,
        };
        self.apply_availability();
    }

    /// Marks the monitor's fixed game mode, so the settings it locks explain that.
    pub fn set_game_mode(&self, mode: Option<&'static str>) {
        self.game_mode.set(mode);
        self.apply_availability();
    }

    /// Applies a fixed game mode on top of the last reading's subtitle and sensitivity.
    fn apply_availability(&self) {
        let base = self.base.borrow();
        let locked = if self.setting.locked_in_game_presets {
            self.game_mode.get()
        } else {
            None
        };
        if let Some(mode) = locked {
            let note = format!("Not available in {mode} game mode; switch to Off or a Gamer preset");
            let subtitle = if base.subtitle.is_empty() {
                note
            } else {
                format!("{} — {note}", base.subtitle)
            };
            self.row.set_sensitive(false);
            self.row.set_subtitle(&glib::markup_escape_text(&subtitle));
        } else {
            self.row.set_sensitive(base.sensitive);
            self.row.set_subtitle(&glib::markup_escape_text(&base.subtitle));
        }
    }

    /// The slider moved: write its value once it rests for [`DEBOUNCE`].
    fn slid(self: &Rc<Self>, value: u16) {
        if let Some(old) = self.debounce.take() {
            old.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(DEBOUNCE, move || {
            if let Some(row) = weak.upgrade() {
                row.debounce.take(); // this source has fired; don't remove it later
                row.send(value);
            }
        });
        self.debounce.replace(Some(source));
    }

    fn send(&self, value: u16) {
        self.status.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.start();
        let id = (self.on_change)(self.setting, value);
        self.wanted.set(Some((id, value)));
    }

    /// Shows the result of request `id`. Returns false if a newer value is already on its way,
    /// in which case this result is stale and ignored.
    pub fn written(&self, id: u64, result: &Result<WriteStatus, String>) -> bool {
        let Some((_, value)) = self.wanted.get().filter(|(w, _)| *w == id) else {
            return false;
        };
        if self.debounce.borrow().is_some() {
            return false;
        }
        self.wanted.set(None);
        self.spinner.stop();
        self.spinner.set_visible(false);
        let sent = self.describe(value);
        self.status_for.set(match result {
            Ok(WriteStatus::Verified) => Some(value),
            Ok(WriteStatus::Mismatch(got)) => Some(*got),
            _ => None,
        });
        let (icon, tip) = match result {
            Ok(WriteStatus::Verified) => ("object-select-symbolic", format!("The monitor confirmed {sent}")),
            Ok(WriteStatus::Mismatch(got)) => {
                let got_text = self.describe(*got);
                self.show(&Ok(Reading {
                    value: *got,
                    max: self.max(),
                }));
                (
                    "dialog-warning-symbolic",
                    format!("Sent {sent}, but the monitor reports {got_text}"),
                )
            }
            Ok(WriteStatus::Unknown(e)) => (
                "dialog-question-symbolic",
                format!("Sent {sent}, but couldn't read it back: {e}"),
            ),
            Err(e) => ("dialog-warning-symbolic", format!("Couldn't change it: {e}")),
        };
        self.status.set_icon_name(Some(icon));
        self.status.set_tooltip_text(Some(&tip));
        self.status.set_visible(true);
        true
    }

    /// Maps a combo-row position to the raw value the monitor uses.
    fn combo_value(&self, picked: u32) -> Option<u16> {
        match self.setting.domain {
            Domain::Enum(_) => Some(picked as u16),
            Domain::Values(values) => values.get(picked as usize).map(|(value, _)| *value),
            Domain::Range { .. } => None,
        }
    }

    /// The slider's current upper bound, to redisplay a read-back value within it.
    fn max(&self) -> u16 {
        self.scale.as_ref().map_or(u16::MAX, |s| s.adjustment().upper() as u16)
    }

    /// "Office" for named values, "55" for ranges.
    pub fn describe(&self, value: u16) -> String {
        self.setting
            .label_for(value)
            .map_or_else(|| value.to_string(), str::to_string)
    }
}
