//! The watcher script is JavaScript `KWin` evaluates, and the receiver is a zbus interface the
//! script calls by name. Nothing compiles one against the other, and a mismatch fails without an
//! error on either side: `KWin` sends a call nothing answers and the pin button stops following.

use std::path::Path;

use tokio::sync::watch;
use zbus::object_server::Interface;

use super::{KeepAboveReports, REPORT_METHOD, REPORT_PATH, keep_above_plugin, keep_above_script};

const PID: u32 = 4242;
const BUS_NAME: &str = ":1.77";

fn introspection() -> String {
    let (reports, _reported) = watch::channel(false);
    let mut xml = String::new();
    KeepAboveReports { reports }.introspect_to_writer(&mut xml, 0);
    xml
}

#[test]
fn the_receiver_serves_the_method_the_script_calls() {
    let xml = introspection();

    assert!(
        xml.contains(&format!("<method name=\"{REPORT_METHOD}\">")),
        "the `#[zbus(name)]` on `changed` drifted from `REPORT_METHOD`:\n{xml}"
    );
}

#[test]
fn the_script_calls_back_to_this_connection_on_the_served_path_and_interface() {
    let script = keep_above_script(PID, BUS_NAME);

    let expected = format!(
        "callDBus(\"{BUS_NAME}\", \"{REPORT_PATH}\", \"{}\", \"{REPORT_METHOD}\", window.keepAbove);",
        KeepAboveReports::name()
    );
    assert!(script.contains(&expected), "no `{expected}` in the script:\n{script}");
}

/// zbus hands each call its own task unless told otherwise, and two quick titlebar toggles then
/// reach the button in either order, leaving it on the state the window no longer has.
#[test]
fn reports_are_handled_in_the_order_kwin_sends_them() {
    let (reports, _reported) = watch::channel(false);

    assert!(!KeepAboveReports { reports }.spawn_tasks_for_methods());
}

/// The report goes to Melodia's own bus name, so a window of any other process would tell this
/// one about a pin it never had.
#[test]
fn the_script_follows_only_this_process() {
    let script = keep_above_script(PID, BUS_NAME);

    assert!(script.contains(&format!("if (window.pid !== {PID}) {{")), "{script}");
}

/// Plasma 5 names the signal `clientAdded`, and a hide to the tray re-creates the window, so
/// connecting `windowAdded` alone stops following on Plasma 5 and on Plasma 6 after the first hide.
#[test]
fn the_script_tracks_windows_added_later_under_either_plasma_name() {
    let script = keep_above_script(PID, BUS_NAME);

    assert!(
        script.contains("(workspace.windowAdded || workspace.clientAdded).connect(track);"),
        "{script}"
    );
}

/// A relaunch unloads the previous run's watcher by name before loading its own. A name that moved
/// between launches would leave the old script behind, calling a bus name nobody owns.
#[test]
fn a_data_root_names_its_watcher_the_same_on_every_launch() {
    let root = Path::new("/home/user/.local/share/Melodia");

    assert_eq!(keep_above_plugin(root), keep_above_plugin(root));
}

/// A dev build and an installed one run side by side on separate roots. Sharing a name, each
/// launch would unload the other's watcher.
#[test]
fn two_data_roots_name_separate_watchers() {
    let installed = keep_above_plugin(Path::new("/home/user/.local/share/Melodia"));
    let dev = keep_above_plugin(Path::new("/home/user/.local/share/Melodia-dev"));

    assert_ne!(installed, dev);
}
