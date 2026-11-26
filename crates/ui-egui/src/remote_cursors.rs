//! Where the other people of a shared workbook are: the sheet each one has open and the cells each
//! has selected, as the parent page sends them (`collab-cursors`). Drawn on the grid (an outline,
//! a light fill and a name tag on each range) and on the sheet tabs (a coloured dot per person).
//! Draw-only: nothing here takes input and nothing here touches the workbook, its history or its
//! unsaved-changes state.
//!
//! The parent supplies the name and colour (the same ones as its participant list); this side only
//! checks and draws them. A malformed entry is skipped, never an error.
//!
//! The place of this editor goes the other way (`collab-cursor`): the open sheet and the current
//! selected range, sent only when it changed.

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, pos2, vec2};
use gridcraft_engine::core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_engine::model::Sheet;
use serde_json::{Value, json};

use crate::grid::{Geo, outline_bands, range_in_pane};
use crate::theme;

/// Most people drawn: a longer list is cut (a hostile or broken parent can't make a frame slow).
pub const MAX_CURSORS: usize = 200;
/// Longest name drawn, in characters.
pub const MAX_NAME_CHARS: usize = 40;
/// Name tags stacked on one spot (or dots on one tab) before "+N".
const MAX_TAGS: usize = 4;
/// Grey for a colour that isn't `#rrggbb`.
const FALLBACK: [u8; 3] = [128, 128, 128];

/// One other connection's place in the workbook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCursor {
    pub conn: u64,
    pub name: String,
    pub color: [u8; 3],
    /// Only looking (role `view`): its name tag has a dashed border.
    pub view: bool,
    /// The sheet's index in the workbook.
    pub sheet: usize,
    pub range: RangeRef,
}

/// `#rrggbb` as bytes.
fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// The place in a `collab-cursor` / `collab-cursors` entry: `sheet:<index>` and `{r1, c1, r2, c2}`.
/// `None` for another region, an unknown shape, a flipped range or one beyond the sheet's size.
pub fn parse_place(region: &str, pos: &Value) -> Option<(usize, RangeRef)> {
    let digits = region.strip_prefix("sheet:")?;
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sheet = digits.parse::<usize>().ok()?;
    let n = |k: &str| pos.get(k)?.as_u64().and_then(|v| u32::try_from(v).ok());
    let (r1, c1, r2, c2) = (n("r1")?, n("c1")?, n("r2")?, n("c2")?);
    if r1 > r2 || c1 > c2 || r2 >= MAX_ROWS || c2 >= MAX_COLS {
        return None;
    }
    Some((sheet, RangeRef { start: CellRef::new(r1, c1), end: CellRef::new(r2, c2) }))
}

/// The people in a `collab-cursors` message that have a grid place, in order. Entries of another
/// region or with a place that can't be read are left out; the rest are kept.
pub fn parse(m: &Value) -> Vec<RemoteCursor> {
    let Some(list) = m.get("cursors").and_then(Value::as_array) else { return vec![] };
    list.iter().take(MAX_CURSORS).filter_map(parse_one).collect()
}

fn parse_one(c: &Value) -> Option<RemoteCursor> {
    let (sheet, range) = parse_place(c.get("region")?.as_str()?, c.get("pos")?)?;
    let conn = c.get("conn")?.as_u64()?;
    let name: String = c.get("name").and_then(Value::as_str).unwrap_or_default().chars().filter(|ch| !ch.is_control()).take(MAX_NAME_CHARS).collect();
    let color = c.get("color").and_then(Value::as_str).and_then(parse_color).unwrap_or(FALLBACK);
    let view = c.get("role").and_then(Value::as_str) == Some("view");
    Some(RemoteCursor { conn, name, color, view, sheet, range })
}

/// The shared-editing state of the app: whether it was opened with `cursors`, the others' places
/// and the place last sent.
#[derive(Clone, Debug)]
pub struct Remote {
    enabled: bool,
    /// The others (the parent sends the whole list each time).
    pub list: Vec<RemoteCursor>,
    /// What the parent last heard: `Some(None)` = nothing (cleared), `None` = not known (a new open).
    last: Option<Option<(usize, RangeRef)>>,
}

impl Default for Remote {
    fn default() -> Self {
        Self { enabled: false, list: vec![], last: Some(None) }
    }
}

impl Remote {
    /// A host `open` was taken: with or without `cursors`. The others' places of the earlier
    /// workbook are dropped (the parent sends the new ones) and the next place is sent again.
    pub fn opened(&mut self, on: bool) {
        self.enabled = on;
        self.list.clear();
        if on {
            self.last = None;
        }
    }

    /// The list the parent sent; ignored unless the workbook was opened with `cursors`.
    pub fn receive(&mut self, m: &Value) {
        if self.enabled {
            self.list = parse(m);
        }
    }

    /// The `collab-cursor` body (`region`, and `pos` unless it is cleared) when the place differs
    /// from the one last sent. Without `cursors` nothing is ever sent, except one clearing after
    /// an open turned it off.
    pub fn outgoing(&mut self, place: Option<(usize, RangeRef)>) -> Option<Value> {
        let want = if self.enabled { place } else { None };
        if self.last.as_ref() == Some(&want) {
            return None;
        }
        self.last = Some(want);
        Some(match want {
            Some((sheet, r)) => {
                json!({"region": format!("sheet:{sheet}"), "pos": {"r1": r.start.row, "c1": r.start.col, "r2": r.end.row, "c2": r.end.col}})
            }
            None => json!({"region": null}),
        })
    }

    /// The last place could not be sent: the next frame sends it again.
    pub fn forget_sent(&mut self) {
        self.last = None;
    }

    /// The people who have sheet `i` open (for its tab).
    fn on_sheet(&self, i: usize) -> impl Iterator<Item = &RemoteCursor> {
        self.list.iter().filter(move |c| c.sheet == i)
    }
}

fn color(c: &RemoteCursor, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.color[0], c.color[1], c.color[2], alpha)
}

/// Black or white, whichever reads on `c`.
fn on(c: [u8; 3]) -> Color32 {
    let l = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    if l > 150.0 { Color32::BLACK } else { Color32::WHITE }
}

fn label(c: &RemoteCursor) -> &str {
    if c.name.is_empty() { "?" } else { c.name.as_str() }
}

/// A name tag whose bottom-left corner is at `at`. `row` stacks tags that share a spot upward.
fn tag(p: &Painter, at: Pos2, c: &RemoteCursor, row: usize) {
    let galley = p.layout_no_wrap(label(c).to_string(), theme::ui_font(11.0), on(c.color));
    let size = galley.size() + vec2(8.0, 4.0);
    let r = Rect::from_min_size(pos2(at.x, at.y - size.y * (row as f32 + 1.0)), size);
    p.rect_filled(r, CornerRadius::same(3), color(c, 255));
    if c.view {
        // A viewer's tag has a dashed border (no icon): it is told apart from an editor's by shape.
        let b = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        p.extend(egui::Shape::dashed_line(&b, Stroke::new(1.0, on(c.color)), 3.0, 2.0));
    }
    p.galley(r.min + vec2(4.0, 2.0), galley, on(c.color));
}

/// The others' ranges on sheet `si` in one pane of the grid (`rows`, `cols` and the painter's clip
/// are the pane's): a light fill, a 2px outline and, in the pane that has the range's first cell,
/// a name tag. Ranges that are off screen are skipped.
pub(crate) fn paint(p: &Painter, geo: &Geo, sh: &Sheet, si: usize, all: &[RemoteCursor], rows: (u32, u32), cols: (u32, u32)) {
    // Tags on the same first cell stack; how many each spot has so far.
    let mut stacked: Vec<(CellRef, usize)> = vec![];
    for c in all.iter().filter(|c| c.sheet == si && range_in_pane(c.range, rows, cols)) {
        let clip = p.clip_rect();
        let rect = geo.range_rect(sh, c.range);
        if !rect.is_finite() || !rect.intersects(clip) {
            continue;
        }
        p.rect_filled(rect.intersect(clip), 0.0, color(c, 36));
        for side in outline_bands(rect.expand(0.5), 2.0, clip) {
            p.rect_filled(side, 0.0, color(c, 255));
        }
        if !range_in_pane(RangeRef::cell(c.range.start), rows, cols) {
            continue;
        }
        let n = match stacked.iter_mut().find(|(s, _)| *s == c.range.start) {
            Some(s) => {
                s.1 += 1;
                s.1 - 1
            }
            None => {
                stacked.push((c.range.start, 1));
                0
            }
        };
        // Above the range; on the first row (nothing above in this pane) inside its top edge.
        let above = rect.top() - clip.top() >= 18.0;
        let at = if above { pos2(rect.left(), rect.top()) } else { pos2(rect.left(), rect.top() + 18.0) };
        if n < MAX_TAGS {
            tag(p, at, c, n);
        } else if n == MAX_TAGS {
            let extra = all.iter().filter(|o| o.sheet == si && o.range.start == c.range.start).count() - MAX_TAGS;
            p.text(pos2(at.x, at.y - 18.0 * MAX_TAGS as f32), Align2::LEFT_BOTTOM, format!("+{extra}"), theme::ui_font(11.0), Color32::GRAY);
        }
    }
}

/// What a sheet tab shows of the others: a dot per person on it (at most [`MAX_TAGS`]) and the
/// names for its tooltip. `None` when nobody is there.
pub struct TabMark {
    pub dots: Vec<(Color32, bool)>,
    pub more: usize,
    pub names: String,
}

pub(crate) fn tab_mark(remote: &Remote, sheet: usize) -> Option<TabMark> {
    let here: Vec<&RemoteCursor> = remote.on_sheet(sheet).collect();
    if here.is_empty() {
        return None;
    }
    Some(TabMark {
        dots: here.iter().take(MAX_TAGS).map(|c| (color(c, 255), c.view)).collect(),
        more: here.len().saturating_sub(MAX_TAGS),
        names: here.iter().map(|c| label(c)).collect::<Vec<_>>().join(", "),
    })
}

/// The dots of `mark` along the top-right of the tab `r`.
pub(crate) fn paint_tab_mark(p: &Painter, r: Rect, mark: &TabMark) {
    for (i, (col, view)) in mark.dots.iter().enumerate() {
        let at = pos2(r.right() - 8.0 - 10.0 * i as f32, r.top() + 7.0);
        p.circle(at, 4.0, *col, Stroke::new(1.0, Color32::WHITE));
        if *view {
            p.circle_stroke(at, 5.5, Stroke::new(1.0, *col));
        }
    }
    if mark.more > 0 {
        p.text(
            pos2(r.right() - 8.0 - 10.0 * MAX_TAGS as f32, r.top() + 7.0),
            Align2::RIGHT_CENTER,
            format!("+{}", mark.more),
            theme::ui_font(10.0),
            Color32::GRAY,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(conn: u64, region: &str, pos: Value) -> Value {
        json!({"conn": conn, "user": "u", "name": "Kim", "color": "#ff8800", "role": "edit", "region": region, "pos": pos})
    }

    fn at(r1: u32, c1: u32, r2: u32, c2: u32) -> Value {
        json!({"r1": r1, "c1": c1, "r2": r2, "c2": c2})
    }

    fn rng(r1: u32, c1: u32, r2: u32, c2: u32) -> RangeRef {
        RangeRef { start: CellRef::new(r1, c1), end: CellRef::new(r2, c2) }
    }

    #[test]
    fn reads_grid_places_and_skips_the_rest() {
        let m = json!({"cursors": [
            entry(1, "sheet:2", at(1, 2, 3, 4)),
            entry(2, "doc", json!({})),
            entry(3, "sheet:x", at(0, 0, 0, 0)),
            entry(4, "sheet:0", json!({"r1": 0})),
            entry(5, "sheet:0", json!([1, 2])),
            json!({"conn": 6, "region": "sheet:0"}),
            json!("junk"),
        ]});
        let got = parse(&m);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].conn, got[0].sheet, got[0].range), (1, 2, rng(1, 2, 3, 4)));
        assert_eq!(got[0].color, [255, 136, 0]);
        assert!(!got[0].view);
    }

    #[test]
    fn places_outside_the_sheet_or_flipped_are_not_drawn() {
        let bad = [
            at(5, 0, 4, 0),
            at(0, 5, 0, 4),
            at(0, 0, MAX_ROWS, 0),
            at(0, 0, 0, MAX_COLS),
            json!({"r1": -1, "c1": 0, "r2": 0, "c2": 0}),
            json!({"r1": 0.5, "c1": 0, "r2": 1, "c2": 0}),
            json!({"r1": 4294967296u64, "c1": 0, "r2": 4294967296u64, "c2": 0}),
        ];
        for pos in bad {
            assert!(parse_place("sheet:0", &pos).is_none(), "{pos}");
        }
        assert_eq!(parse_place("sheet:0", &at(0, 0, MAX_ROWS - 1, MAX_COLS - 1)), Some((0, rng(0, 0, MAX_ROWS - 1, MAX_COLS - 1))));
        for region in ["sheet:", "sheet:-1", "sheet:1.5", "sheet:99999999999", "Sheet:1", "sheet:1/x"] {
            assert!(parse_place(region, &at(0, 0, 0, 0)).is_none(), "{region}");
        }
    }

    #[test]
    fn guards_the_size_name_and_colour() {
        let many: Vec<Value> = (0..300).map(|i| entry(i, "sheet:0", at(0, 0, 0, 0))).collect();
        assert_eq!(parse(&json!({"cursors": many})).len(), MAX_CURSORS);
        let long = json!({"conn": 1, "name": "x".repeat(500), "color": "red", "role": "view", "region": "sheet:1", "pos": at(0, 0, 1, 1)});
        let got = parse(&json!({"cursors": [long]}));
        assert_eq!(got[0].name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(got[0].color, FALLBACK);
        assert!(got[0].view);
        // Not a list: nothing, never a panic.
        assert!(parse(&json!({"cursors": 5})).is_empty());
        assert!(parse(&json!({})).is_empty());
        // Control characters can't ride in a name.
        let ctl = json!({"conn": 1, "name": "a\u{0}b\nc", "region": "sheet:0", "pos": at(0, 0, 0, 0)});
        assert_eq!(parse(&json!({"cursors": [ctl]}))[0].name, "abc");
    }

    #[test]
    fn colours_parse_only_as_hex() {
        assert_eq!(parse_color("#00ff7F"), Some([0, 255, 127]));
        for bad in ["00ff7f", "#fff", "#gggggg", "#00ff7f00", ""] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_place_goes_out_once_per_change() {
        let mut r = Remote::default();
        r.opened(true);
        let a = Some((1, rng(2, 3, 4, 5)));
        let m = r.outgoing(a).expect("the first place is sent");
        assert_eq!(m["region"], "sheet:1");
        assert_eq!(m["pos"], at(2, 3, 4, 5));
        assert!(r.outgoing(a).is_none(), "the same place isn't sent twice");
        assert!(r.outgoing(Some((1, rng(2, 3, 4, 6)))).is_some());
        assert!(r.outgoing(Some((0, rng(2, 3, 4, 6)))).is_some(), "another sheet is a change");
        let gone = r.outgoing(None).expect("the place is cleared");
        assert!(gone["region"].is_null() && gone.get("pos").is_none());
        assert!(r.outgoing(None).is_none());
        // A new open sends the place again even when it is the same.
        let b = Some((0, rng(0, 0, 0, 0)));
        r.outgoing(b);
        r.opened(true);
        assert!(r.outgoing(b).is_some());
    }

    #[test]
    fn nothing_goes_out_or_comes_in_without_the_option() {
        let mut r = Remote::default();
        assert!(r.outgoing(Some((0, rng(0, 0, 0, 0)))).is_none());
        assert!(r.outgoing(None).is_none());
        r.receive(&json!({"cursors": [entry(1, "sheet:0", at(0, 0, 0, 0))]}));
        assert!(r.list.is_empty(), "the others' places are ignored too");
        r.opened(false);
        r.receive(&json!({"cursors": [entry(1, "sheet:0", at(0, 0, 0, 0))]}));
        assert!(r.list.is_empty());
    }

    #[test]
    fn an_open_without_the_option_clears_a_place_sent_before() {
        let mut r = Remote::default();
        r.opened(true);
        r.outgoing(Some((0, rng(1, 1, 1, 1))));
        r.opened(false);
        assert!(r.outgoing(Some((0, rng(1, 1, 1, 1)))).is_some_and(|m| m["region"].is_null()), "cleared once");
        assert!(r.outgoing(Some((0, rng(1, 1, 1, 1)))).is_none());
    }

    #[test]
    fn the_others_are_taken_whole_and_cleared_by_an_open() {
        let mut r = Remote::default();
        r.opened(true);
        r.receive(
            &json!({"cursors": [entry(1, "sheet:0", at(0, 0, 0, 0)), entry(2, "sheet:1", at(1, 1, 2, 2)), entry(3, "sheet:1", at(3, 3, 3, 3))]}),
        );
        assert_eq!(r.list.len(), 3);
        assert_eq!(tab_mark(&r, 1).map(|m| m.dots.len()), Some(2));
        assert!(tab_mark(&r, 5).is_none());
        r.receive(&json!({"cursors": []}));
        assert!(r.list.is_empty(), "an empty list clears the others");
        r.receive(&json!({"cursors": [entry(1, "sheet:0", at(0, 0, 0, 0))]}));
        r.opened(true);
        assert!(r.list.is_empty(), "a new workbook starts without them");
    }
}
