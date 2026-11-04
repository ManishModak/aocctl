//! The main window: finds AOC monitors, explains why one can't be reached, and shows the
//! picked monitor's settings and presets. All monitor and file I/O goes through the worker.

use crate::access::{self, Access};
use crate::setting_row::{OnChange, SettingRow};
use crate::worker::{self, Job, Reply, Values};
use adw::prelude::*;
use aocctl::ddc::Reading;
use aocctl::detect;
use aocctl::models::{MODE_SETTINGS, Setting};
use aocctl::monitor::{Monitor, WriteStatus};
use aocctl::presets::{self, Outcome, Preset};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

const README_PERMISSIONS: &str = "https://github.com/ManishModak/aocctl#permissions";
const MODEL_REPORT: &str = "https://github.com/ManishModak/aocctl/issues/new?template=model-report.yml";

/// Settings page groups, by setting id; settings not listed go under "Other".
const GROUPS: &[(&str, &[&str])] = &[
    (
        "Picture",
        &["brightness", "contrast", "dcr", "hdr-mode", "eco", "clear-vision"],
    ),
    (
        "Gaming",
        &[
            "overdrive",
            "shadow-control",
            "game-color",
            "frame-counter",
            "dial-point",
        ],
    ),
    ("Eye Comfort", &["low-blue"]),
];

pub struct Window {
    pub window: adw::ApplicationWindow,
    header: adw::HeaderBar,
    title: adw::WindowTitle,
    switcher: adw::ViewSwitcher,
    picker: gtk::DropDown,
    picker_suppress: Cell<bool>,
    toasts: adw::ToastOverlay,
    pages: gtk::Stack,
    status: adw::StatusPage,
    banner: adw::Banner,
    settings_page: adw::PreferencesPage,
    presets_group: adw::PreferencesGroup,
    jobs: mpsc::Sender<Job>,
    /// Id of the last write request sent.
    last_write: Cell<u64>,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    /// Connected AOC monitors, in picker order.
    monitors: Vec<Monitor>,
    current: Option<Monitor>,
    /// Connector id whose settings rows are built.
    built_for: Option<String>,
    groups: Vec<adw::PreferencesGroup>,
    rows: Vec<Rc<SettingRow>>,
    presets: Vec<Preset>,
    preset_rows: Vec<gtk::Widget>,
}

impl Window {
    pub fn new(app: &adw::Application) -> Rc<Window> {
        let (jobs, job_rx) = mpsc::channel();
        let (reply_tx, replies) = async_channel::unbounded();
        std::thread::spawn(move || worker::run(job_rx, reply_tx));

        let title = adw::WindowTitle::new("aocctl", "");
        let switcher = adw::ViewSwitcher::new();
        switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        let picker = gtk::DropDown::from_strings(&[]);
        picker.set_tooltip_text(Some("Monitor"));
        picker.set_visible(false);
        header.pack_start(&picker);
        let menu = gio::Menu::new();
        menu.append(Some("_Refresh"), Some("app.refresh"));
        menu.append(Some("_About aocctl"), Some("app.about"));
        let menu_button = gtk::MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");
        menu_button.set_menu_model(Some(&menu));
        menu_button.set_tooltip_text(Some("Main Menu"));
        menu_button.set_primary(true);
        header.pack_end(&menu_button);
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Refresh"));
        refresh.set_action_name(Some("app.refresh"));
        header.pack_end(&refresh);

        let status = adw::StatusPage::new();
        let banner = adw::Banner::new("");
        banner.set_use_markup(false);
        let settings_page = adw::PreferencesPage::new();
        let presets_page = adw::PreferencesPage::new();
        let presets_group = adw::PreferencesGroup::new();
        presets_group.set_title("Presets");
        presets_page.add(&presets_group);
        let save = gtk::Button::with_mnemonic("_Save Current…");
        save.add_css_class("flat");
        presets_group.set_header_suffix(Some(&save));
        let view = adw::ViewStack::new();
        view.add_titled_with_icon(&settings_page, Some("settings"), "Settings", "video-display-symbolic");
        view.add_titled_with_icon(&presets_page, Some("presets"), "Presets", "view-list-bullet-symbolic");
        view.set_vexpand(true);
        switcher.set_stack(Some(&view));
        let monitor_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        monitor_box.append(&banner);
        monitor_box.append(&view);
        let pages = gtk::Stack::new();
        pages.add_named(&status, Some("status"));
        pages.add_named(&monitor_box, Some("monitor"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&pages));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::new(app);
        window.set_title(Some("aocctl"));
        window.set_default_size(640, 760);
        window.set_content(Some(&toolbar));

        let win = Rc::new(Window {
            window,
            header,
            title,
            switcher,
            picker,
            picker_suppress: Cell::new(false),
            toasts,
            pages,
            status,
            banner,
            settings_page,
            presets_group,
            jobs,
            last_write: Cell::new(0),
            state: RefCell::default(),
        });
        let weak = Rc::downgrade(&win);
        win.picker.connect_selected_notify(move |p| {
            if let Some(w) = weak.upgrade().filter(|w| !w.picker_suppress.get()) {
                w.select(p.selected() as usize);
            }
        });
        let weak = Rc::downgrade(&win);
        save.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.ask_preset_name();
            }
        });
        let weak = Rc::downgrade(&win);
        win.banner.connect_button_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.explain_model_report();
            }
        });
        let weak = Rc::downgrade(&win);
        glib::spawn_future_local(async move {
            while let Ok(reply) = replies.recv().await {
                match weak.upgrade() {
                    Some(w) => w.handle(reply),
                    None => break,
                }
            }
        });
        win.discover();
        win
    }

    fn send(&self, job: Job) {
        // The worker only stops when this sender is dropped, so sending can't fail.
        let _ = self.jobs.send(job);
    }

    fn current(&self) -> Option<Monitor> {
        self.state.borrow().current.clone()
    }

    fn is_current(&self, connector: &str) -> bool {
        self.state
            .borrow()
            .current
            .as_ref()
            .is_some_and(|m| m.connector.id == connector)
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(&glib::markup_escape_text(text)));
    }

    /// Looks for monitors again (startup, Refresh, Try Again).
    pub fn discover(&self) {
        self.loading("Looking for Monitors");
        self.send(Job::Discover);
    }

    fn handle(self: &Rc<Self>, reply: Reply) {
        match reply {
            Reply::Monitors(all) => self.found(all),
            Reply::Values { connector, result } if self.is_current(&connector) => self.values(&connector, result),
            Reply::Written {
                connector,
                setting,
                id,
                result,
            } if self.is_current(&connector) => self.written(setting, id, result),
            Reply::Presets { connector, result } if self.is_current(&connector) => self.show_presets(result),
            Reply::Saved { connector, result } if self.is_current(&connector) => self.saved(result),
            Reply::Applied {
                connector,
                name,
                result,
            } if self.is_current(&connector) => self.applied(&name, result),
            Reply::Deleted { connector, result } if self.is_current(&connector) => self.deleted(result),
            _ => {} // about a monitor that's no longer shown
        }
    }

    fn found(self: &Rc<Self>, all: Vec<Monitor>) {
        let aoc: Vec<Monitor> = all.iter().filter(|m| m.is_aoc()).cloned().collect();
        let previous = self.current().map(|m| m.connector.id);
        {
            let mut st = self.state.borrow_mut();
            st.monitors = aoc.clone();
            st.current = None;
        }
        if aoc.is_empty() {
            self.picker.set_visible(false);
            self.title.set_subtitle("");
            let others: Vec<String> = all
                .iter()
                .map(|m| {
                    let e = &m.connector.edid;
                    let model = e.model.as_deref().unwrap_or("monitor");
                    format!("{}: {} {model} is not an AOC monitor", m.connector.name, e.mfg)
                })
                .collect();
            let (title, description) = if others.is_empty() {
                (
                    "No External Monitor Found",
                    "Connect an AOC monitor over DisplayPort or HDMI, then try again.".to_string(),
                )
            } else {
                ("No AOC Monitor Found", others.join("\n"))
            };
            self.show_status("video-display-symbolic", title, &description, &[]);
            return;
        }
        let index = aoc
            .iter()
            .position(|m| Some(&m.connector.id) == previous.as_ref())
            .or_else(|| aoc.iter().position(|m| m.bus.is_some()))
            .unwrap_or(0);
        let labels: Vec<String> = aoc.iter().map(monitor_label).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.picker_suppress.set(true);
        self.picker.set_model(Some(&gtk::StringList::new(&labels)));
        self.picker.set_selected(index as u32);
        self.picker_suppress.set(false);
        self.picker.set_visible(aoc.len() > 1);
        self.select(index);
    }

    /// Shows the monitor at `index` of the picker: its settings, or why it can't be reached.
    fn select(self: &Rc<Self>, index: usize) {
        let Some(m) = self.state.borrow().monitors.get(index).cloned() else {
            return;
        };
        self.title.set_subtitle(&monitor_label(&m));
        {
            let mut st = self.state.borrow_mut();
            st.current = Some(m.clone());
            st.built_for = None;
        }
        if m.bus.is_some() {
            self.loading("Reading Settings");
            self.send(Job::ReadAll(m.clone()));
            self.send(Job::ListPresets(m));
            return;
        }
        let devices: Vec<_> = m.connector.buses.iter().map(|b| detect::dev_path(*b)).collect();
        let fix: &[(&str, Action)] = &[
            ("How to Fix", Action::Open(README_PERMISSIONS)),
            ("Try Again", Action::Retry),
        ];
        match access::check(&devices) {
            Access::NoDevices => self.show_status(
                "dialog-password-symbolic",
                "Monitor Access Is Off",
                "The i2c-dev kernel module isn't loaded, so aocctl can't talk to monitors. \
                 The aocctl package loads it at boot; until then, run “sudo modprobe i2c-dev”.",
                fix,
            ),
            Access::Denied(dev) => self.show_status(
                "dialog-password-symbolic",
                "No Access to the Monitor",
                &format!(
                    "Your user can't open {}. Install aocctl's udev rule (the aocctl package does), \
                     then log out and back in.",
                    dev.display()
                ),
                fix,
            ),
            Access::Granted => self.show_status(
                "computer-fail-symbolic",
                &format!("{} Doesn't Answer", monitor_label(&m)),
                &format!(
                    "No DDC/CI answer: {}.\n\nCheck that DDC/CI is on in the monitor's own menu and that no other \
                     program is controlling it. NVIDIA's driver may not support DDC/CI over DisplayPort.",
                    m.error.as_deref().unwrap_or("no i2c bus found for it")
                ),
                &[("Try Again", Action::Retry)],
            ),
        }
    }

    fn loading(&self, title: &str) {
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(32, 32);
        spinner.start();
        self.status.set_icon_name(None);
        self.status.set_title(title);
        self.status.set_description(None);
        self.status.set_child(Some(&spinner));
        self.show_page("status");
    }

    fn show_status(self: &Rc<Self>, icon: &str, title: &str, description: &str, buttons: &[(&str, Action)]) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(gtk::Align::Center);
        let buttons: &[(&str, Action)] = if buttons.is_empty() {
            &[("Try Again", Action::Retry)]
        } else {
            buttons
        };
        for (i, (label, action)) in buttons.iter().enumerate() {
            let b = gtk::Button::with_label(label);
            b.add_css_class("pill");
            if i + 1 == buttons.len() {
                b.add_css_class("suggested-action");
            }
            let (weak, action) = (Rc::downgrade(self), *action);
            b.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    match action {
                        Action::Retry => w.discover(),
                        Action::Open(url) => w.open_url(url),
                    }
                }
            });
            row.append(&b);
        }
        self.status.set_icon_name(Some(icon));
        self.status.set_title(title);
        self.status
            .set_description(Some(&glib::markup_escape_text(description)));
        self.status.set_child(Some(&row));
        self.show_page("status");
    }

    fn show_page(&self, name: &str) {
        self.pages.set_visible_child_name(name);
        if name == "monitor" {
            self.header.set_title_widget(Some(&self.switcher));
        } else {
            self.header.set_title_widget(Some(&self.title));
        }
    }

    pub fn open_url(&self, url: &str) {
        gtk::UriLauncher::new(url).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
    }

    fn values(self: &Rc<Self>, connector: &str, result: Result<Values, String>) {
        let built = self.state.borrow().built_for.as_deref() == Some(connector);
        match result {
            Err(e) if built => self.toast(&format!("Couldn't read the settings: {e}")),
            Err(e) => self.show_status("computer-fail-symbolic", "Couldn't Read the Settings", &e, &[]),
            Ok(values) if built => {
                let rows = self.state.borrow().rows.clone();
                for (s, r) in &values {
                    if let Some(row) = rows.iter().find(|row| row.setting.id == s.id) {
                        row.show(r);
                    }
                }
            }
            Ok(values) => {
                self.build_settings(&values);
                self.state.borrow_mut().built_for = Some(connector.to_string());
                self.show_page("monitor");
            }
        }
    }

    fn build_settings(self: &Rc<Self>, values: &[(&'static Setting, Result<Reading, String>)]) {
        let old = std::mem::take(&mut self.state.borrow_mut().groups);
        for g in old {
            self.settings_page.remove(&g);
        }
        // Rows write to the monitor they were built for, even if another is picked meanwhile.
        let built_for = self.current();
        let weak = Rc::downgrade(self);
        let on_change: OnChange = Rc::new(move |setting, value| {
            let (Some(w), Some(monitor)) = (weak.upgrade(), built_for.clone()) else {
                return 0;
            };
            let id = w.last_write.get() + 1;
            w.last_write.set(id);
            w.send(Job::Write {
                monitor,
                setting,
                value,
                id,
            });
            id
        });
        // (group, position in it) of a setting; settings in no group go last, in model order.
        let place = |id: &str| {
            GROUPS
                .iter()
                .enumerate()
                .find_map(|(g, (_, ids))| Some((g, ids.iter().position(|i| *i == id)?)))
                .unwrap_or((GROUPS.len(), 0))
        };
        let mut values: Vec<_> = values.iter().collect();
        values.sort_by_key(|(s, _)| place(s.id));
        let mut groups: Vec<adw::PreferencesGroup> = Vec::new();
        let mut rows = Vec::new();
        for g in 0..=GROUPS.len() {
            let group = adw::PreferencesGroup::new();
            group.set_title(GROUPS.get(g).map_or("Other", |(title, _)| title));
            let mut empty = true;
            for (s, r) in values.iter().filter(|(s, _)| place(s.id).0 == g) {
                let row = SettingRow::new(s, r, on_change.clone());
                group.add(&row.row);
                rows.push(row);
                empty = false;
            }
            if !empty {
                self.settings_page.add(&group);
                groups.push(group);
            }
        }
        let m = self.current();
        match m.as_ref().filter(|m| m.profile.is_none()) {
            Some(m) => {
                self.banner.set_title(&format!(
                    "aocctl doesn't know the {} yet: only brightness and contrast are available",
                    model_name(m)
                ));
                self.banner.set_button_label(Some("How to Help"));
                self.banner.set_revealed(true);
            }
            None => self.banner.set_revealed(false),
        }
        if let Some(m) = &m {
            let shown = format!("{} on {}", model_name(m), m.connector.name);
            self.settings_page.set_description(&glib::markup_escape_text(&shown));
            self.presets_group
                .set_description(Some(&glib::markup_escape_text(&format!(
                    "Saved for every {}, and shared with the aocctl command.",
                    model_name(m)
                ))));
        }
        let mut st = self.state.borrow_mut();
        st.groups = groups;
        st.rows = rows;
    }

    fn written(&self, setting: &'static Setting, id: u64, result: Result<WriteStatus, String>) {
        let row = self
            .state
            .borrow()
            .rows
            .iter()
            .find(|r| r.setting.id == setting.id)
            .cloned();
        if !row.is_some_and(|r| r.written(id, &result)) {
            return;
        }
        if let Err(e) = &result {
            self.toast(&format!("Couldn't change {}: {e}", setting.label));
        }
        // A failed write may have left the control off the monitor's value, and a mode
        // switch changes other settings: read everything again.
        if result.is_err() || MODE_SETTINGS.contains(&setting.id) {
            self.reread();
        }
    }

    fn reread(&self) {
        if let Some(m) = self.current() {
            self.send(Job::ReadAll(m));
        }
    }

    fn show_presets(self: &Rc<Self>, result: Result<Vec<Preset>, String>) {
        let old = std::mem::take(&mut self.state.borrow_mut().preset_rows);
        for r in old {
            self.presets_group.remove(&r);
        }
        let mut rows: Vec<gtk::Widget> = Vec::new();
        let list = match result {
            Ok(list) => list,
            Err(e) => {
                let row = adw::ActionRow::new();
                row.set_title("Couldn't load presets");
                row.set_subtitle(&glib::markup_escape_text(&e));
                row.add_css_class("error");
                rows.push(row.upcast());
                Vec::new()
            }
        };
        if list.is_empty() && rows.is_empty() {
            let row = adw::ActionRow::new();
            row.set_title("No presets yet");
            row.set_subtitle("Set the monitor up the way you like, then save it as a preset.");
            rows.push(row.upcast());
        }
        for p in &list {
            rows.push(self.preset_row(p).upcast());
        }
        for r in &rows {
            self.presets_group.add(r);
        }
        let mut st = self.state.borrow_mut();
        st.presets = list;
        st.preset_rows = rows;
    }

    fn preset_row(self: &Rc<Self>, p: &Preset) -> adw::ActionRow {
        let row = adw::ActionRow::new();
        row.set_title(&glib::markup_escape_text(&p.name));
        let subtitle = match p.problems.as_slice() {
            [] => summary(&p.values),
            problems => format!("Can't be applied: {}", problems.join("; ")),
        };
        row.set_subtitle(&glib::markup_escape_text(&subtitle));
        let apply = gtk::Button::with_label("Apply");
        apply.set_valign(gtk::Align::Center);
        apply.set_sensitive(p.problems.is_empty());
        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_valign(gtk::Align::Center);
        delete.add_css_class("flat");
        delete.set_tooltip_text(Some("Delete Preset"));
        row.add_suffix(&apply);
        row.add_suffix(&delete);
        let (weak, name) = (Rc::downgrade(self), p.name.clone());
        apply.connect_clicked(move |b| {
            if let Some(w) = weak.upgrade()
                && let Some(monitor) = w.current()
            {
                b.set_sensitive(false);
                b.set_label("Applying…");
                w.send(Job::ApplyPreset {
                    monitor,
                    name: name.clone(),
                });
            }
        });
        let (weak, name) = (Rc::downgrade(self), p.name.clone());
        delete.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.confirm_delete(&name);
            }
        });
        row
    }

    fn ask_preset_name(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some("Save Current Settings"),
            Some("Saves the monitor's current settings as a preset."),
        );
        let entry = adw::EntryRow::new();
        entry.set_title("Name");
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&entry);
        dialog.set_extra_child(Some(&list));
        dialog.add_responses(&[("cancel", "_Cancel"), ("save", "_Save")]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        dialog.set_response_enabled("save", false);
        let d = dialog.clone();
        entry.connect_changed(move |e| d.set_response_enabled("save", presets::check_name(e.text().trim()).is_ok()));
        entry.set_activates_default(true);
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("save"), move |_, _| {
            let Some(w) = weak.upgrade() else { return };
            let name = entry.text().trim().to_string();
            let existing = w
                .state
                .borrow()
                .presets
                .iter()
                .find(|p| p.name.to_lowercase() == name.to_lowercase())
                .map(|p| p.name.clone());
            match existing {
                Some(old) => w.confirm_replace(&old),
                None => w.save_preset(name),
            }
        });
        dialog.present(Some(&self.window));
    }

    fn confirm_replace(self: &Rc<Self>, name: &str) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Replace “{name}”?")),
            Some("Its values are replaced with the monitor's current settings."),
        );
        dialog.add_responses(&[("cancel", "_Cancel"), ("replace", "_Replace")]);
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let (weak, name) = (Rc::downgrade(self), name.to_string());
        dialog.connect_response(Some("replace"), move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.save_preset(name.clone());
            }
        });
        dialog.present(Some(&self.window));
    }

    fn save_preset(&self, name: String) {
        if let Some(monitor) = self.current() {
            self.send(Job::SavePreset { monitor, name });
        }
    }

    fn confirm_delete(self: &Rc<Self>, name: &str) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Delete “{name}”?")),
            Some("The aocctl command and widgets lose it too."),
        );
        dialog.add_responses(&[("cancel", "_Cancel"), ("delete", "_Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let (weak, name) = (Rc::downgrade(self), name.to_string());
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(w) = weak.upgrade()
                && let Some(monitor) = w.current()
            {
                w.send(Job::DeletePreset {
                    monitor,
                    name: name.clone(),
                });
            }
        });
        dialog.present(Some(&self.window));
    }

    fn list_presets(&self) {
        if let Some(m) = self.current() {
            self.send(Job::ListPresets(m));
        }
    }

    fn saved(&self, result: Result<(String, Vec<&'static Setting>), String>) {
        match result {
            Ok((name, skipped)) if skipped.is_empty() => self.toast(&format!("Saved “{name}”")),
            Ok((name, skipped)) => {
                let left: Vec<&str> = skipped.iter().map(|s| s.label).collect();
                self.toast(&format!(
                    "Saved “{name}” without {} (set by another setting)",
                    left.join(", ")
                ));
            }
            Err(e) => self.alert("Couldn't Save the Preset", &e),
        }
        self.list_presets();
    }

    fn applied(&self, name: &str, result: Result<Vec<(&'static Setting, u16, Outcome)>, String>) {
        match result {
            Ok(results) => {
                let failed: Vec<String> = results
                    .iter()
                    .filter(|(_, _, o)| !o.ok())
                    .map(|(s, v, o)| failure(s, *v, o))
                    .collect();
                if !failed.is_empty() {
                    self.alert(&format!("“{name}” Was Only Partly Applied"), &failed.join("\n"));
                } else if results.iter().all(|(_, _, o)| *o == Outcome::Unchanged) {
                    self.toast(&format!("“{name}” is already set"));
                } else {
                    self.toast(&format!("Applied “{name}”"));
                }
            }
            Err(e) => self.alert(&format!("Couldn't Apply “{name}”"), &e),
        }
        self.reread();
        self.list_presets();
    }

    fn deleted(&self, result: Result<String, String>) {
        match result {
            Ok(name) => self.toast(&format!("Deleted “{name}”")),
            Err(e) => self.alert("Couldn't Delete the Preset", &e),
        }
        self.list_presets();
    }

    fn alert(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("close", "_Close");
        dialog.present(Some(&self.window));
    }

    fn explain_model_report(self: &Rc<Self>) {
        let model = self.current().map_or_else(|| "monitor".to_string(), |m| model_name(&m));
        let dialog = adw::AlertDialog::new(
            Some(&format!("Help Add the {model}")),
            Some(
                "aocctl needs a report from your monitor to support its other settings. \
                 In a terminal, run\n\naocctl probe --aoc-items\n\nand paste its output into a model report. \
                 It only reads values and leaves out your serial number.",
            ),
        );
        dialog.add_responses(&[("close", "_Close"), ("report", "_Open Model Report")]);
        dialog.set_response_appearance("report", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("close");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("report"), move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.open_url(MODEL_REPORT);
            }
        });
        dialog.present(Some(&self.window));
    }
}

#[derive(Clone, Copy)]
enum Action {
    Retry,
    Open(&'static str),
}

/// "AOC 24G4"
fn model_name(m: &Monitor) -> String {
    let e = &m.connector.edid;
    format!("{} {}", e.mfg, e.model.as_deref().unwrap_or("monitor"))
}

/// "AOC 24G4 (DP-1)", as the picker and window subtitle show a monitor.
fn monitor_label(m: &Monitor) -> String {
    format!("{} ({})", model_name(m), m.connector.name)
}

/// "Brightness 10 · Low Blue mode Office"
fn summary(values: &[(&'static Setting, u16)]) -> String {
    values
        .iter()
        .map(|(s, v)| {
            format!(
                "{} {}",
                s.label,
                s.label_for(*v).map_or_else(|| v.to_string(), str::to_string)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One line of the "partly applied" dialog.
fn failure(s: &Setting, v: u16, o: &Outcome) -> String {
    let name = |v: u16| s.label_for(v).map_or_else(|| v.to_string(), str::to_string);
    match o {
        Outcome::Mismatch(got) => format!("{}: set {}, but it reads back {}", s.label, name(v), name(*got)),
        Outcome::Unknown(e) => format!("{}: set {}, but couldn't read it back: {e}", s.label, name(v)),
        Outcome::Failed(e) => format!("{}: {e}", s.label),
        Outcome::Unchanged | Outcome::Verified => format!("{}: {}", s.label, name(v)),
    }
}
