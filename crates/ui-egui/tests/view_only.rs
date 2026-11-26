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
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1800.0, 800.0)).build_ui_state(
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
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![], view, ..Default::default() });
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
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![("file.print".into(), true)], view: read_only(), ..Default::default() });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    let before = h.state().session.host_policy().unwrap().overrides();
    assert_eq!(before, vec![("file.print".to_string(), true)]);
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![("file.print".into(), false), ("home.bold".into(), false)], view: ViewOptions::default(), ..Default::default() });
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
            "Paste (⌘V)", "Align Left", "Center", "Insert", "Delete", "Format", "Fill", "Clear", "Sort & Filter",
            "Conditional Formatting", "Format as Table", "Cell Styles", "formulas.autoSum"][..], &["edit.copy", "Find & Select"][..]),
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

fn click_at(h: &mut Harness, pos: egui::Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }
    h.run_steps(2);
}

/// A workbook with a table (A1:B3) and a chart, so the Table Design and Chart Design tabs have something to show.
fn xlsx_table_chart() -> Vec<u8> {
    let mut s = Session::new();
    s.new_workbook();
    for (cell, v) in [("A1", "k"), ("B1", "v"), ("A2", "a"), ("B2", "1"), ("A3", "b"), ("B3", "2")] {
        s.execute("cell.set", json!({"cell": cell, "input": v})).unwrap();
    }
    s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "type": "column"})).unwrap();
    let b64 = s.execute("file.saveBytes", json!({})).unwrap()["base64"].as_str().unwrap().to_string();
    gridcraft_engine::io::base64_decode(&b64).unwrap()
}

/// What a control may change in a read-only workbook: how it is shown (the workbook stores the view mode and zoom).
const VIEW_STATE: [&str; 6] = ["view.zoom", "view.normal", "view.pageBreakPreview", "view.pageLayout", "view.zoom100", "view.zoomToSelection"];

/// Presses every control of `tab` (the list is what the ribbon really drew), confirms a dialog it opens with
/// Enter, and checks that the workbook is the same one afterwards, whether the control is washed out or not:
/// the only commands that ran are ones a read-only document permits, and the workbook changed (as a value)
/// only for view-state commands. Returns (pressed, washed out).
fn press_tab(h: &mut Harness, tab: &str, chart: Option<u32>) -> (usize, usize) {
    h.state_mut().ui.ribbon_tab = tab.into();
    h.state_mut().selected_chart = chart;
    h.run_steps(3);
    assert!(!h.state().ribbon_overflow, "{tab}: the ribbon is wider than its card, so controls are cut off and not tested");
    let controls = h.state().ribbon_controls.clone();
    assert!(!controls.is_empty() || tab == "Automate", "{tab}: the ribbon drew no controls");
    let (mut pressed, mut washed) = (0, 0);
    for (rect, off) in controls {
        h.state_mut().ui.ribbon_tab = tab.into();
        h.state_mut().selected_chart = chart;
        h.state_mut().dialog = None;
        h.run_steps(2);
        let wb = h.state().session.active().unwrap().wb.clone();
        let before = h.state().session.journal.len();
        click_at(h, rect.center());
        assert!(!off || h.state().dialog.is_none(), "{tab} control at {rect:?} is washed out but opened a dialog");
        if h.state().dialog.is_some() {
            h.key_press(Key::Enter);
            h.run_steps(3);
        }
        let ran: Vec<String> = h.state().session.journal.iter().skip(before).map(|(id, _)| id.clone()).collect();
        let doc = h.state().session.active().unwrap();
        for id in &ran {
            assert!(gridcraft_engine::hostlock::read_only_permits(id), "{tab} control at {rect:?} ran `{id}`, which a read-only document refuses");
        }
        let view_only = !ran.is_empty() && ran.iter().all(|id| VIEW_STATE.contains(&id.as_str()));
        // The workbook is the same one, unless a view-state command ran (and nothing else did).
        assert!(std::sync::Arc::ptr_eq(&wb, &doc.wb) || view_only, "{tab} control at {rect:?} (washed out: {off}) changed the workbook; ran {ran:?}");
        assert!(!off || ran.is_empty() || ran.iter().all(|id| VIEW_STATE.contains(&id.as_str())), "{tab} control at {rect:?} is washed out but ran {ran:?}");
        assert!(h.state().message.is_none(), "{tab} control at {rect:?} showed a message: {:?}", h.state().message);
        washed += usize::from(off);
        pressed += 1;
        h.key_press(Key::Escape);
        h.key_press(Key::Escape);
        h.state_mut().dialog = None;
        h.run_steps(2);
    }
    (pressed, washed)
}

/// The whole ribbon of a read-only workbook, tab by tab and control by control: pressing a control, and
/// confirming the dialog it opens, never changes the workbook; the edit controls are all washed out.
#[test]
fn every_ribbon_control_of_a_read_only_workbook_is_inert_and_the_edit_ones_are_washed_out() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, read_only());
    let (mut pressed, mut washed) = (0, 0);
    for tab in ["Home", "Insert", "Draw", "Page Layout", "Formulas", "Data", "Review", "View", "Automate"] {
        let (p, w) = press_tab(&mut h, tab, None);
        pressed += p;
        washed += w;
    }
    assert!(pressed > 100 && washed > 60, "pressed {pressed} controls, {washed} of them washed out");
}

/// The tabs that appear with the selection (a table cell, a chart) are read-only too.
#[test]
fn the_table_and_chart_design_tabs_of_a_read_only_workbook_are_inert_and_washed_out() {
    let (mut h, inbox, _) = harness();
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx_table_chart()));
    h.state_mut().open_queue.push_back(HostOpen { overrides: vec![], view: read_only(), ..Default::default() });
    h.run_steps(4);
    let chart = h.state().session.active().unwrap().wb.active().unwrap().charts.first().map(|c| c.id);
    assert!(chart.is_some(), "the test workbook has a chart");
    let (p, w) = press_tab(&mut h, "Table Design", None);
    assert!(p >= 3 && w >= 3, "Table Design: pressed {p}, washed out {w}");
    let (p, w) = press_tab(&mut h, "Chart Design", chart);
    assert!(p >= 3 && w >= 3, "Chart Design: pressed {p}, washed out {w}");
}

/// An editable document keeps its labels as they were: a command the host switched off dims the icon alone;
/// only a read-only document washes the text label (Wrap Text, Merge & Center) too.
#[test]
fn an_editable_document_keeps_its_labels_while_a_read_only_one_washes_them() {
    let off = vec![("home.wrapText".to_string(), false), ("home.mergeCenter".to_string(), false)];
    for (view, washed) in [(ViewOptions::default(), false), (read_only(), true)] {
        let (mut h, inbox, _) = harness();
        h.state_mut().open_queue.push_back(HostOpen { overrides: off.clone(), view, ..Default::default() });
        inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
        h.run_steps(4);
        let app = h.state();
        for id in ["home.wrapText", "home.mergeCenter"] {
            assert!(app.switched_off(id), "{id} is switched off");
            assert_eq!(app.label_washed(id), washed, "{id}: the label is washed only in a read-only document");
        }
    }
}

/// The dialogs a read-only document refuses to open are the editing ones of an editable document, as before.
#[test]
fn an_editable_document_still_opens_the_editing_dialogs() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    for name in ["formatCells", "sort", "comment", "insertFunction"] {
        h.state_mut().dialog = None;
        h.state_mut().open_dialog(name, json!({}));
        h.run_steps(2);
        assert!(h.state().dialog.is_some(), "the `{name}` dialog opens in an editable document");
    }
    host_open(&mut h, &inbox, read_only());
    h.state_mut().dialog = None;
    h.state_mut().open_dialog("formatCells", json!({}));
    assert!(h.state().dialog.is_none(), "and not in a read-only one");
}

/// `keepView`: the new workbook replaces the open one and shows its sheet, cell, zoom and scroll again.
#[test]
fn keep_view_open_replaces_in_place_and_restores_the_view() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    {
        let app = h.state_mut();
        app.session.execute("view.zoom", json!({"percent": 150})).unwrap();
        app.session.active_mut().unwrap().selection = gridcraft_engine::Selection::at(CellRef::new(4, 2));
        let k = app.view_key().unwrap();
        app.views.insert(k, gridcraft_ui_egui::SheetView { scroll: egui::vec2(0.0, 120.0) });
    }
    let before = h.state().session.active().unwrap().selection.clone();
    assert_eq!(before.active, CellRef::new(4, 2));
    h.state_mut().open_queue.push_back(HostOpen { keep_view: true, ..Default::default() });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.session.documents().len(), 1, "the old workbook is gone");
    let d = app.session.active().unwrap();
    assert_eq!(d.wb.active().unwrap().zoom, 150);
    assert_eq!(d.selection, before);
    assert!(!d.is_dirty(), "putting the view back is no edit");
    assert_eq!(app.view().scroll, egui::vec2(0.0, 120.0));
    // Without keepView the next open is one more workbook, as before.
    host_open(&mut h, &inbox, ViewOptions::default());
    assert_eq!(h.state().session.documents().len(), 2);
}

fn others() -> serde_json::Value {
    json!({"cursors": [
        {"conn": 1, "user": "a", "name": "Kim", "color": "#ff8800", "role": "edit", "region": "sheet:0", "pos": {"r1": 2, "c1": 1, "r2": 4, "c2": 3}},
        {"conn": 2, "user": "b", "name": "Lee", "color": "#0088ff", "role": "view", "region": "sheet:0", "pos": {"r1": 0, "c1": 0, "r2": 0, "c2": 0}},
        {"conn": 3, "user": "c", "name": "Park", "color": "#00cc66", "role": "edit", "region": "sheet:7", "pos": {"r1": 0, "c1": 0, "r2": 1048576, "c2": 0}}]})
}

#[test]
fn cursors_open_sends_the_place_once_and_draws_the_others_without_an_edit() {
    let (mut h, inbox, _) = harness();
    h.state_mut().open_queue.push_back(HostOpen { cursors: true, ..Default::default() });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    let first = h.state_mut().cursor_message().expect("the first place is sent");
    assert_eq!(first["region"], "sheet:0");
    assert_eq!(first["pos"], json!({"r1": 0, "c1": 0, "r2": 0, "c2": 0}));
    assert!(h.state_mut().cursor_message().is_none(), "the same place isn't sent twice");
    // A moved selection is one more message.
    h.state_mut().session.active_mut().unwrap().selection = gridcraft_engine::Selection::range(gridcraft_engine::core::RangeRef::new(CellRef::new(1, 1), CellRef::new(3, 2)));
    let moved = h.state_mut().cursor_message().expect("a moved selection is sent");
    assert_eq!(moved["pos"], json!({"r1": 1, "c1": 1, "r2": 3, "c2": 2}));
    // The others are drawn (one entry is out of range and skipped) and nothing counts as an edit.
    let (rev, dirty) = {
        let d = h.state().session.active().unwrap();
        (d.revision, d.is_dirty())
    };
    h.state_mut().remote.receive(&others());
    assert_eq!(h.state().remote.list.len(), 2);
    h.run_steps(4);
    let d = h.state().session.active().unwrap();
    assert_eq!((d.revision, d.is_dirty()), (rev, dirty));
    assert!(d.undo.is_empty());
    // A new open drops them.
    h.state_mut().open_queue.push_back(HostOpen { cursors: true, ..Default::default() });
    inbox.lock().unwrap().push(("book.xlsx".into(), xlsx()));
    h.run_steps(4);
    assert!(h.state().remote.list.is_empty());
}

#[test]
fn without_cursors_nothing_is_sent_and_the_others_are_ignored() {
    let (mut h, inbox, _) = harness();
    host_open(&mut h, &inbox, ViewOptions::default());
    assert!(h.state_mut().cursor_message().is_none());
    h.state_mut().remote.receive(&others());
    assert!(h.state().remote.list.is_empty());
    h.run_steps(2);
    assert!(h.state_mut().cursor_message().is_none());
}
