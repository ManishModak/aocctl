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

/// Called with a value the user picked; the window sends it to the worker.
pub type OnChange = Rc<dyn Fn(&'static Setting, u16)>;

pub struct SettingRow {
    pub setting: &'static Setting,
    pub row: adw::ActionRow,
    scale: Option<gtk::Scale>,
    combo: Option<adw::ComboRow>,
    status: gtk::Image,
    spinner: gtk::Spinner,
    /// Set while the code (not the user) moves the control, so that doesn't write.
    suppress: Cell<bool>,
    /// The value last sent whose result hasn't come back yet.
    wanted: Cell<Option<u16>>,
    /// The value the result icon is about; a reading of another value hides the icon.
    status_for: Cell<Option<u16>>,
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
                {
                    row.send(picked as u16);
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
                self.row.set_sensitive(false);
                self.row
                    .set_subtitle(&glib::markup_escape_text(&format!("Couldn't read it: {e}")));
                return;
            }
        };
        self.row.set_sensitive(self.setting.verified_write);
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
            _ => unreachable!("rows are built from their setting's domain"),
        }
        self.suppress.set(false);
        self.row.set_subtitle(&glib::markup_escape_text(&subtitle));
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
        self.wanted.set(Some(value));
        self.status.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.start();
        (self.on_change)(self.setting, value);
    }

    /// Shows a write's result. Returns false if a newer value is already on its way,
    /// in which case this result is stale and ignored.
    pub fn written(&self, value: u16, result: &Result<WriteStatus, String>) -> bool {
        if self.wanted.get() != Some(value) || self.debounce.borrow().is_some() {
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
