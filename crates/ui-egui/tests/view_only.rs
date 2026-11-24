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
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![("file.print".into(), true)], view: read_only() });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    let before = h.state().session.host_policy().unwrap().overrides();
    assert_eq!(before, vec![("file.print".to_string(), true)]);
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![("file.print".into(), false), ("home.bold".into(), false)], view: ViewOptions::default() });
    inbox.lock().unwrap().push(("broken.xlsx".into(), b"PK\x03\x04 not really a workbook".to_vec()));
    h.run_steps(4);
    assert_eq!(h.state().session.documents().len(), 1);
    assert!(h.state().session.read_only(), "the document that stayed open keeps its options");
    assert_eq!(h.state().session.host_policy().unwrap().overrides(), before, "the policy of the failed open is put back");
}

#[test]
fn chrome_false_hides_the_editing_chrome_and_keeps_the_sheet_tabs_and_zoom() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    assert!(h.query_by_label("Paste").is_some(), "the ribbon is there by default");
    host_open(&mut h, &inbox, ViewOptions { chrome: false, ..ViewOptions::default() });
    assert!(h.query_by_label("Paste").is_none(), "the ribbon is gone");
    assert!(h.query_by_label("All sheets").is_some(), "the sheet tabs stay");
    assert!(h.query_by_label("100%").is_none(), "zoomControls defaults to false, like word and deck");
    host_open(&mut h, &inbox, ViewOptions { chrome: false, zoom_controls: true, ..ViewOptions::default() });
    assert!(h.query_by_label("100%").is_some(), "zoomControls keeps the zoom buttons");
}

#[test]
fn a_read_only_find_dialog_has_no_replace_tab() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    h.state_mut().open_dialog("find", json!({"replace": true}));
    h.run_steps(3);
    assert!(h.query_by_label("Replace All").is_some(), "a plain document replaces");
    h.state_mut().dialog = None;
    host_open(&mut h, &inbox, read_only());
    h.state_mut().open_dialog("find", json!({"replace": true}));
    h.run_steps(3);
    assert!(h.query_by_label("Find All").is_some(), "the Find tab is there");
    assert!(h.query_by_label("Replace").is_none(), "the Replace tab is not offered");
    assert!(h.query_by_label("Replace All").is_none());
}

#[test]
fn cancelling_the_password_prompt_puts_the_opens_options_back() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    let before = h.state().session.host_policy().unwrap().overrides();
    // What `logic` leaves behind when a host open stops at the password prompt.
    h.state_mut().session.set_policy(&[("file.print".to_string(), true)]);
    h.state_mut().session.set_next_view(Some(read_only()));
    h.state_mut().password_restore = Some(Some(before.clone()));
    h.state_mut().open_dialog("openPassword", json!({"name": "locked.xlsx", "base64": ""}));
    h.run_steps(3);
    h.input_mut().events.extend(key(Key::Escape, Modifiers::NONE));
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.host_policy().unwrap().overrides(), before, "the policy goes back");
    // The abandoned options must not leak into the next open.
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    assert!(!h.state().session.read_only(), "the next open is the plain editor");
}

#[test]
fn a_read_only_workbook_opens_no_text_box_editor() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    let id = h.state_mut().session.run("insert.textBox", json!({"text": "hi"})).unwrap()["shape"].as_u64().unwrap() as u32;
    h.state_mut().session.active_mut().unwrap().view.read_only = true;
    assert!(!h.state_mut().begin_text_box_edit(id));
    assert!(h.state().text_box_editor.is_none());
    h.state_mut().session.active_mut().unwrap().view.read_only = false;
    assert!(h.state_mut().begin_text_box_edit(id), "the same shape of a plain document does open");
}

#[test]
fn chrome_false_alone_leaves_editing_but_not_comments() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions { chrome: false, ..ViewOptions::default() });
    assert!(!h.state().session.read_only());
    // Typing edits (readOnly was not asked for) ...
    click_cell(&mut h, "B2", 1);
    h.input_mut().events.push(Event::Text("x".into()));
    h.run_steps(3);
    assert!(h.state().editor.is_some());
    // ... but there is no way to write a comment or a note, and no task pane.
    h.state_mut().open_dialog("comment", json!({"threaded": true}));
    h.state_mut().open_dialog("comment", json!({"threaded": false}));
    h.run_steps(2);
    assert!(h.state().dialog.is_none(), "the comment dialog must not open");
    h.state_mut().session.comments_pane = true;
    h.run_steps(3);
    assert!(h.query_by_label("Comments").is_none(), "the comments pane must stay hidden");
    h.state_mut().session.active_mut().unwrap().view.chrome = true;
    h.run_steps(3);
    assert!(h.query_by_label("Comments").is_some(), "the same pane shows once the chrome is back");
}

/// Every ribbon button that edits is washed out in a read-only document (the ones that only look or
/// copy are not), and pressing it changes nothing.
#[test]
fn a_read_only_ribbon_washes_out_every_edit_button_and_pressing_them_changes_nothing() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    assert!(h.state().ribbon_dimmed.is_empty(), "an editable document's ribbon is not washed out");
    host_open(&mut h, &inbox, read_only());
    let wb = h.state().session.active().unwrap().wb.clone();
    let expect = [
        // (tab, buttons that are washed out, buttons that stay bright)
        ("Home", &["edit.cut", "home.bold", "home.fontName", "home.mergeCenter", "home.fillColor", "home.alignTop",
            "Paste (⌘V)", "Align Left", "Center", "Merge & Center", "Insert", "Delete", "Format", "Fill", "Clear", "Sort & Filter",
            "Conditional Formatting", "Format as Table", "Cell Styles", "AutoSum (⌘⇧T)"][..], &["edit.copy", "Find & Select"][..]),
        ("Insert", &["insert.table", "insert.textBox", "Shapes", "Insert a picture"][..], &[][..]),
        ("Data", &["data.sortAscending", "data.filter", "Data Validation"][..], &[][..]),
        ("Formulas", &["formulas.autoSum", "Use in Formula"][..], &["Trace Precedents", "Trace Dependents", "Remove Arrows", "Watch Window"][..]),
        ("Review", &["review.deleteComment"][..], &["Previous Comment", "Next Comment", "Comments pane"][..]),
    ];
    for (tab, dimmed, bright) in expect {
        h.state_mut().ui.ribbon_tab = tab.into();
        h.run_steps(3);
        let got = h.state().ribbon_dimmed.clone();
        for d in dimmed {
            assert!(got.iter().any(|g| g == d), "{tab}: `{d}` should be washed out, got {got:?}");
        }
        for b in bright {
            assert!(!got.iter().any(|g| g == b), "{tab}: `{b}` stays bright, got {got:?}");
        }
    }
    // Pressing them: the commands behind the buttons are refused, and so is a click on Paste.
    for id in ["edit.cut", "edit.paste", "home.bold", "home.alignLeft", "home.mergeCenter", "home.fillColor", "home.insertRows", "data.sortAscending", "home.numberFormat"] {
        let err = h.state_mut().run(id, json!({})).expect_err(id);
        assert!(err.contains(gridcraft_engine::hostlock::READ_ONLY_TEXT), "`{id}` should be refused as read-only, got {err}");
    }
    h.state_mut().ui.ribbon_tab = "Home".into();
    h.run_steps(3);
    h.get_by_label("Paste").click();
    h.run_steps(3);
    assert!(std::sync::Arc::ptr_eq(&wb, &h.state().session.active().unwrap().wb), "nothing the buttons do reaches the workbook");
    assert!(h.state().session.active().unwrap().undo.is_empty());
    host_open(&mut h, &inbox, ViewOptions::default());
    h.run_steps(3);
    assert!(h.state().ribbon_dimmed.is_empty(), "an open without options washes nothing out");
}
