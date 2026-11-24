//! A workbook the host opens with `readOnly` / `chrome` / `zoomControls`: no input reaches the
//! workbook, nothing turns dirty or gets saved, and a file opened without options is the plain editor again.
//! The engine side (the command lists) is tested in `hostlock.rs` and `tests/read_only.rs`.

use std::sync::{Arc, Mutex};

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::core::CellRef;
use gridcraft_engine::{Session, ViewOptions};
use gridcraft_ui_egui::{HostOpen, Inbox, Services, SheetApp, grid::Geo};
use serde_json::json;

type Downloads = Arc<Mutex<Vec<String>>>;
type Harness = egui_kittest::Harness<'static, SheetApp>;

fn xlsx() -> Vec<u8> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "hello"})).unwrap();
    let b64 = s.execute("file.saveBytes", json!({})).unwrap()["base64"].as_str().unwrap().to_string();
    gridcraft_engine::io::base64_decode(&b64).unwrap()
}

fn harness() -> (Harness, Inbox, Downloads) {
    let inbox = Inbox::default();
    let downloads = Downloads::default();
    let sink = downloads.clone();
    let services = Services {
        download: Some(Box::new(move |name, _| sink.lock().unwrap().push(name.to_string()))),
        inbox: Some(inbox.clone()),
        ..Default::default()
    };
    let mut session = Session::new();
    session.lock_host();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, services),
    );
    h.run_steps(4);
    (h, inbox, downloads)
}

/// What the host's open message does: the options are queued, the file comes through the inbox.
fn host_open(h: &mut Harness, inbox: &Inbox, view: ViewOptions) {
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![], view });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
}

fn read_only() -> ViewOptions {
    ViewOptions { read_only: true, ..ViewOptions::default() }
}

fn key(key: Key, modifiers: Modifiers) -> [Event; 2] {
    [
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
    ]
}

fn click_cell(h: &mut Harness, cell: &str, times: usize) {
    let app = h.state();
    let sheet = app.session.active().unwrap().wb.active().unwrap();
    let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
    let pos = geo.cell_rect(sheet, CellRef::parse(cell).unwrap()).center();
    h.input_mut().events.push(Event::PointerMoved(pos));
    for _ in 0..times {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

fn nothing_changed(h: &Harness, wb: &Arc<gridcraft_engine::model::Workbook>) {
    let app = h.state();
    assert!(app.editor.is_none(), "no cell editor may open");
    assert!(app.message.is_none() && app.toast.is_none(), "refusals are quiet");
    let doc = app.session.active().unwrap();
    assert!(Arc::ptr_eq(&doc.wb, wb), "the workbook must be untouched");
    assert!(!doc.is_dirty());
    assert!(doc.undo.is_empty());
}

#[test]
fn no_input_edits_a_read_only_workbook() {
    let (mut h, inbox, downloads) = harness();
    host_open(&mut h, &inbox, read_only());
    assert!(h.state().session.read_only());
    click_cell(&mut h, "B2", 1);
    let wb = h.state().session.active().unwrap().wb.clone();
    // Typing, IME text, F2, double click, Delete, Backspace, cut, paste, Save.
    h.input_mut().events.push(Event::Text("x".into()));
    h.run_steps(3);
    nothing_changed(&h, &wb);
    for (k, m) in [(Key::F2, Modifiers::NONE), (Key::Delete, Modifiers::NONE), (Key::Backspace, Modifiers::NONE), (Key::Enter, Modifiers::ALT)] {
        h.input_mut().events.extend(key(k, m));
        h.run_steps(3);
        nothing_changed(&h, &wb);
    }
    click_cell(&mut h, "B2", 2);
    nothing_changed(&h, &wb);
    h.input_mut().events.push(Event::Cut);
    h.input_mut().events.push(Event::Paste("pasted".into()));
    h.input_mut().events.push(Event::Ime(egui::ImeEvent::Commit("한글".into())));
    h.run_steps(3);
    nothing_changed(&h, &wb);
    h.input_mut().events.extend(key(Key::S, Modifiers::COMMAND));
    h.input_mut().events.extend(key(Key::Z, Modifiers::COMMAND));
    h.run_steps(3);
    nothing_changed(&h, &wb);
    assert!(downloads.lock().unwrap().is_empty(), "nothing was handed to the host");
}

#[test]
fn a_read_only_workbook_can_still_be_read_and_zoomed() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, read_only());
    click_cell(&mut h, "C3", 1);
    assert_eq!(h.state().session.active().unwrap().selection.active, CellRef::parse("C3").unwrap());
    h.state_mut().run_or_alert("view.zoom", json!({"percent": 150}));
    h.run_steps(2);
    assert_eq!(h.state().session.active().unwrap().wb.active().unwrap().zoom, 150);
    h.input_mut().events.push(Event::Copy);
    h.run_steps(3);
    assert!(h.state().session.clipboard.is_some(), "copy still works");
    assert!(!h.state().session.active().unwrap().is_dirty(), "zooming is no change to save");
    store_view_is_skipped(&mut h);
}

fn store_view_is_skipped(h: &mut Harness) {
    h.state_mut().store_view_state();
    assert!(!h.state().session.active().unwrap().is_dirty());
}

#[test]
fn an_open_without_options_is_the_plain_editor_again() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, read_only());
    // The next open comes with no options of its own: nothing queued means the defaults.
    h.state_mut().open_queue.push_back(HostOpen::default());
    inbox.lock().unwrap().push(("two.xlsx".into(), xlsx()));
    h.run_steps(4);
    assert!(!h.state().session.read_only());
    click_cell(&mut h, "B2", 1);
    h.input_mut().events.push(Event::Text("x".into()));
    h.run_steps(3);
    assert!(h.state().editor.is_some(), "typing opens the editor again");
}

#[test]
fn a_failed_open_keeps_the_earlier_document_as_it_was() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, read_only());
    h.state_mut().open_queue.push_back(HostOpen::default());
    inbox.lock().unwrap().push(("broken.xlsx".into(), b"PK\x03\x04 not really a workbook".to_vec()));
    h.run_steps(4);
    assert_eq!(h.state().session.documents().len(), 1);
    assert!(h.state().session.read_only(), "the document that stayed open keeps its options");
}

#[test]
fn chrome_false_hides_the_editing_chrome_and_keeps_the_sheet_tabs_and_zoom() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    assert!(h.query_by_label("Paste").is_some(), "the ribbon is there by default");
    host_open(&mut h, &inbox, ViewOptions { chrome: false, ..ViewOptions::default() });
    assert!(h.query_by_label("Paste").is_none(), "the ribbon is gone");
    assert!(h.query_by_label("All sheets").is_some(), "the sheet tabs stay");
    assert!(h.query_by_label("100%").is_some(), "the zoom buttons stay");
    host_open(&mut h, &inbox, ViewOptions { chrome: false, zoom_controls: false, ..ViewOptions::default() });
    assert!(h.query_by_label("100%").is_none(), "zoomControls false removes them");
}
