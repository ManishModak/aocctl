//! `aocctl-gtk`: a GTK app for people who'd rather not use the aocctl command. It links
//! the aocctl library directly; every monitor and presets-file operation runs on a
//! worker thread (see `worker`), and the window only shows results.

mod access;
mod setting_row;
mod window;
mod worker;

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

const APP_ID: &str = "io.github.manishmodak.aocctl";

type Slot = Rc<RefCell<Option<Rc<window::Window>>>>;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let window: Slot = Rc::default();
    let w = window.clone();
    app.connect_activate(move |app| {
        let win = w.borrow_mut().get_or_insert_with(|| window::Window::new(app)).clone();
        win.window.present();
    });
    add_actions(&app, window);
    app.run()
}

/// App-wide actions: refresh (Ctrl+R or F5), about, quit (Ctrl+Q).
fn add_actions(app: &adw::Application, window: Slot) {
    let refresh = gio::SimpleAction::new("refresh", None);
    let w = window.clone();
    refresh.connect_activate(move |_, _| {
        if let Some(win) = w.borrow().clone() {
            win.discover();
        }
    });
    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(move |_, _| {
        if let Some(win) = window.borrow().clone() {
            about_dialog().present(Some(&win.window));
        }
    });
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.clone();
    quit.connect_activate(move |_, _| a.quit());
    app.add_action(&refresh);
    app.add_action(&about);
    app.add_action(&quit);
    app.set_accels_for_action("app.refresh", &["<Control>r", "F5"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

fn about_dialog() -> adw::AboutDialog {
    let about = adw::AboutDialog::builder()
        .application_name("aocctl")
        .application_icon(APP_ID)
        .developer_name("Manish Modak")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(
            "Change AOC monitor settings on Linux, including the ones only AOC's G-Menu offers.\n\n\
             An independent project, not made or endorsed by AOC.",
        )
        .website("https://github.com/ManishModak/aocctl")
        .issue_url("https://github.com/ManishModak/aocctl/issues")
        .license_type(gtk::License::Custom)
        .license("MIT or Apache-2.0, at your option.")
        .build();
    about.add_link(
        "Add Your Monitor",
        "https://github.com/ManishModak/aocctl#add-your-monitor",
    );
    about
}
