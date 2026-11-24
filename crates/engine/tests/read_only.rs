//! A document opened read-only (`ViewOptions::read_only`): nothing edits it, viewing never makes it
//! dirty, and an open without options is the plain editor again.

use gridcraft_engine::{EngineError, Session, ViewOptions};
use serde_json::json;

/// A one-cell workbook as base64 xlsx.
fn book() -> String {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "hello"})).unwrap();
    s.execute("file.saveBytes", json!({})).unwrap()["base64"].as_str().unwrap().to_string()
}

fn open(s: &mut Session, view: Option<ViewOptions>) {
    s.set_next_view(view);
    s.execute("file.open", json!({"name": "a.xlsx", "base64": book()})).unwrap();
}

fn read_only() -> ViewOptions {
    ViewOptions { read_only: true, ..ViewOptions::default() }
}

#[test]
fn edits_are_refused_and_leave_the_workbook_alone() {
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    let wb = s.active().unwrap().wb.clone();
    for (id, params) in [
        ("cell.set", json!({"cell": "A1", "input": "x"})),
        ("home.bold", json!({})),
        ("home.insertRows", json!({})),
        ("home.deleteSheet", json!({})),
        ("edit.cut", json!({})),
        ("edit.paste", json!({})),
        ("edit.undo", json!({})),
        ("edit.beginEdit", json!({})),
        ("draw.stroke", json!({})),
        ("file.save", json!({})),
        ("file.saveAs", json!({})),
        ("file.saveBytes", json!({})),
        ("file.exportCsv", json!({"path": "x.csv"})),
        ("sheet.rename", json!({"name": "Z"})),
    ] {
        match s.execute(id, params) {
            Err(e @ EngineError::ReadOnly(_)) => {
                assert!(e.to_string().contains(gridcraft_engine::hostlock::READ_ONLY_TEXT), "the app silences this error by its text");
            }
            other => panic!("`{id}` should be refused as read-only, got {other:?}"),
        }
    }
    assert!(std::sync::Arc::ptr_eq(&wb, &s.active().unwrap().wb));
    assert!(s.active().unwrap().undo.is_empty());
}

#[test]
fn looking_still_works_and_is_never_dirty() {
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    for (id, params) in [
        ("selection.set", json!({"cell": "A1"})),
        ("edit.selectAll", json!({})),
        ("edit.copy", json!({})),
        ("view.zoom", json!({"percent": 150})),
        ("view.zoom100", json!({})),
        ("sheet.read", json!({})),
        ("cell.get", json!({"cell": "A1"})),
    ] {
        s.execute(id, params).unwrap_or_else(|e| panic!("`{id}` should run read-only: {e}"));
    }
    assert!(!s.active().unwrap().is_dirty(), "viewing must not make a read-only document dirty");
}

#[test]
fn an_open_without_options_is_the_plain_editor_again() {
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    open(&mut s, None);
    assert_eq!(s.active().unwrap().view, ViewOptions::default());
    s.execute("cell.set", json!({"cell": "B1", "input": "x"})).unwrap();
    assert!(s.active().unwrap().is_dirty());
}

#[test]
fn the_options_belong_to_the_document() {
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    let first = s.active_index();
    open(&mut s, None);
    s.execute("cell.set", json!({"cell": "B1", "input": "x"})).unwrap();
    s.set_active(first);
    assert!(matches!(s.execute("cell.set", json!({"cell": "B1", "input": "x"})), Err(EngineError::ReadOnly(_))));
}

#[test]
fn a_second_window_of_a_read_only_document_is_read_only() {
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    s.execute("view.newWindow", json!({})).unwrap();
    assert!(s.read_only());
}

#[test]
fn a_session_with_no_options_is_unchanged() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    assert!(s.active().unwrap().is_dirty());
}

#[test]
fn a_host_policy_cannot_turn_an_edit_back_on() {
    let mut s = Session::new();
    s.lock_host();
    // The parent switches on commands that are off by default; none of them is an edit it may force.
    s.set_policy(&[("file.saveAs".to_string(), true), ("data.getData".to_string(), true), ("home.bold".to_string(), true)]);
    open(&mut s, Some(read_only()));
    for id in ["home.bold", "file.saveAs", "data.getData", "edit.paste"] {
        assert!(matches!(s.execute(id, json!({})), Err(EngineError::ReadOnly(_) | EngineError::HostLocked(_))), "`{id}` must stay refused");
    }
    assert!(s.active().unwrap().undo.is_empty());
}

/// `is_dirty` is always false for a read-only document, so a command it still allows that changed the
/// workbook would go unnoticed: run every allowed command and make sure the workbook is the same one.
#[test]
fn every_allowed_command_leaves_the_workbook_alone() {
    use gridcraft_engine::hostlock::READ_ONLY_ALLOWED;
    let mut s = Session::new();
    open(&mut s, Some(read_only()));
    let mut wb = s.active().unwrap().wb.clone();
    // Closing the document or opening another one is not an edit of it (the active document changes).
    let leaves = ["file.close", "file.new", "file.open", "file.openBytes", "view.newWindow"];
    let mut changed = vec![];
    for id in READ_ONLY_ALLOWED.iter().filter(|id| !leaves.contains(id)) {
        // Commands that need parameters or a dialog fail on `{}`: that is fine, a failure edits nothing.
        let _ = s.execute(id, json!({}));
        let now = s.active().unwrap_or_else(|| panic!("`{id}` closed the document")).wb.clone();
        if !std::sync::Arc::ptr_eq(&wb, &now) {
            changed.push(*id);
            wb = now;
        }
    }
    // These only change how the workbook is shown (view mode, zoom), which the workbook stores; a dirty
    // flag is never raised for a read-only document, and nothing is saved.
    let view_state = ["view.normal", "view.pageBreakPreview", "view.pageLayout", "view.zoom100", "view.zoomToSelection"];
    changed.retain(|id| !view_state.contains(id));
    assert!(changed.is_empty(), "allowed commands that edit the workbook: {changed:?}");
}
