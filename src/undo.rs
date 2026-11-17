use std::path::{Path, PathBuf};

pub const DEPTH: usize = 20;

pub enum Step {
    /// `pictures`: the renames the move made, (source name, target name). `kept`: target names of
    /// pictures that were already in the target, which the undo leaves there.
    MovePage { from: PathBuf, to: PathBuf, pictures: Vec<(String, String)>, kept: Vec<String> },
    MoveSection { from: PathBuf, to: PathBuf },
    RenamePage { from: PathBuf, to: PathBuf, old_title: String, new_title: String },
    RenameSection { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf, entry: PathBuf, section: bool },
    Create { path: PathBuf, section: bool, entry: Option<PathBuf> },
}

impl Step {
    /// Records where the item really ended up: after an undo that is the old place (`from`), after a
    /// redo the new one (`to`). Only for the steps that move or rename something.
    pub fn set_result(&mut self, undoing: bool, path: &Path) {
        match self {
            Step::MovePage { from, to, .. }
            | Step::MoveSection { from, to }
            | Step::RenamePage { from, to, .. }
            | Step::RenameSection { from, to } => {
                *(if undoing { from } else { to }) = path.to_path_buf();
            }
            Step::Delete { .. } | Step::Create { .. } => {}
        }
    }
}

/// How often in a row a step may fail as busy before it is dropped (see `UndoLog::return_busy`).
pub const BUSY_TRIES: u32 = 3;

pub struct Record {
    pub step: Step,
    pub label: String,
    /// How often in a row the step failed as busy; 0 after it worked.
    pub busy_tries: u32,
}

#[derive(Default)]
pub struct UndoLog {
    undo: Vec<Record>,
    redo: Vec<Record>,
}

impl UndoLog {
    /// A user action: newest on top, oldest dropped beyond DEPTH, redo cleared.
    pub fn push(&mut self, r: Record) {
        self.push_capped(r);
        self.redo.clear();
    }

    /// Puts `r` on top of the undo list and drops the oldest record beyond DEPTH.
    fn push_capped(&mut self, r: Record) {
        self.undo.push(r);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
    }

    pub fn take_undo(&mut self) -> Option<Record> {
        self.undo.pop()
    }

    pub fn take_redo(&mut self) -> Option<Record> {
        self.redo.pop()
    }

    /// After a successful undo: the record goes onto the redo list.
    pub fn undone(&mut self, mut r: Record) {
        r.busy_tries = 0;
        self.redo.push(r);
    }

    /// After a successful redo: the record goes back onto undo, redo is kept.
    pub fn redone(&mut self, mut r: Record) {
        r.busy_tries = 0;
        self.push_capped(r);
    }

    /// After a step taken from the undo list (`undoing`) or the redo list failed as busy: puts it
    /// back, unless this was its BUSY_TRIES-th busy failure in a row. Windows reports a folder with
    /// broken permissions like one that is only in use ("access denied"), so without a limit such a
    /// step would block the steps under it forever. Returns whether the record was put back.
    pub fn return_busy(&mut self, mut r: Record, undoing: bool) -> bool {
        r.busy_tries += 1;
        if r.busy_tries >= BUSY_TRIES {
            return false;
        }
        if undoing { self.return_undo(r) } else { self.return_redo(r) }
        true
    }

    /// Puts a record taken with `take_undo` back unchanged (the step could not start).
    pub fn return_undo(&mut self, r: Record) {
        self.undo.push(r);
    }

    /// Puts a record taken with `take_redo` back unchanged (the step could not start).
    pub fn return_redo(&mut self, r: Record) {
        self.redo.push(r);
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|r| r.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|r| r.label.as_str())
    }
}

/// Swaps each pair (used to reverse a picture-name mapping).
pub fn invert(pictures: &[(String, String)]) -> Vec<(String, String)> {
    pictures.iter().map(|(a, b)| (b.clone(), a.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(label: &str) -> Record {
        Record {
            step: Step::MoveSection { from: PathBuf::from("a"), to: PathBuf::from("b") },
            label: label.to_string(),
            busy_tries: 0,
        }
    }

    #[test]
    fn set_result_writes_the_real_path() {
        let mut step = Step::RenamePage {
            from: PathBuf::from("X.markdown"),
            to: PathBuf::from("Y.md"),
            old_title: "X".into(),
            new_title: "Y".into(),
        };
        step.set_result(true, Path::new("X.md"));
        step.set_result(false, Path::new("Y.md"));
        let Step::RenamePage { from, to, .. } = &step else { panic!() };
        assert_eq!((from.as_path(), to.as_path()), (Path::new("X.md"), Path::new("Y.md")));
        let mut m = Step::MoveSection { from: PathBuf::from("a"), to: PathBuf::from("b") };
        m.set_result(false, Path::new("b (2)"));
        let Step::MoveSection { from, to } = &m else { panic!() };
        assert_eq!((from.as_path(), to.as_path()), (Path::new("a"), Path::new("b (2)")));
    }

    #[test]
    fn push_caps_at_depth() {
        let mut log = UndoLog::default();
        for i in 0..=DEPTH {
            log.push(rec(&format!("r{i}")));
        }
        let mut n = 0;
        let mut last = String::new();
        while let Some(r) = log.take_undo() {
            n += 1;
            last = r.label;
        }
        assert_eq!(n, DEPTH);
        assert_eq!(last, "r1");
    }

    #[test]
    fn redone_caps_at_depth() {
        let mut log = UndoLog::default();
        for i in 0..DEPTH {
            log.push(rec(&format!("r{i}")));
        }
        log.redone(rec("new"));
        assert_eq!(log.undo_label(), Some("new"));
        let mut n = 0;
        let mut last = String::new();
        while let Some(r) = log.take_undo() {
            n += 1;
            last = r.label;
        }
        assert_eq!(n, DEPTH);
        assert_eq!(last, "r1");
    }

    #[test]
    fn push_clears_redo() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        let r = log.take_undo().unwrap();
        log.undone(r);
        assert_eq!(log.redo_label(), Some("A"));
        log.push(rec("B"));
        assert_eq!(log.redo_label(), None);
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        let r = log.take_undo().unwrap();
        assert_eq!(log.undo_label(), None);
        log.undone(r);
        let r = log.take_redo().unwrap();
        assert_eq!(log.redo_label(), None);
        log.redone(r);
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn redone_keeps_rest_of_redo() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        log.undone(b);
        let a = log.take_undo().unwrap();
        log.undone(a);
        let a = log.take_redo().unwrap();
        log.redone(a);
        assert_eq!(log.redo_label(), Some("B"));
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn drop_failed_keeps_rest() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let _ = log.take_undo();
        assert_eq!(log.undo_label(), Some("A"));
    }

    #[test]
    fn return_undo_restores_unchanged() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        let a = log.take_undo().unwrap();
        log.undone(a);
        log.return_undo(b);
        assert_eq!(log.undo_label(), Some("B"));
        assert_eq!(log.redo_label(), Some("A"));
    }

    /// A step that keeps failing as busy (a folder with broken permissions also reports "access
    /// denied") is dropped on the BUSY_TRIES-th try, so that older steps stay reachable.
    #[test]
    fn return_busy_drops_after_tries() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        for _ in 1..BUSY_TRIES {
            let b = log.take_undo().unwrap();
            assert!(log.return_busy(b, true));
            assert_eq!(log.undo_label(), Some("B"));
        }
        let b = log.take_undo().unwrap();
        assert!(!log.return_busy(b, true));
        assert_eq!(log.undo_label(), Some("A"));
    }

    /// A step that worked starts counting busy tries from zero again.
    #[test]
    fn return_busy_count_resets_after_success() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        for _ in 1..BUSY_TRIES {
            let a = log.take_undo().unwrap();
            assert!(log.return_busy(a, true));
        }
        let a = log.take_undo().unwrap();
        log.undone(a);
        for _ in 1..BUSY_TRIES {
            let a = log.take_redo().unwrap();
            assert!(log.return_busy(a, false));
            assert_eq!(log.redo_label(), Some("A"));
        }
    }

    #[test]
    fn return_redo_restores_unchanged() {
        let mut log = UndoLog::default();
        log.push(rec("A"));
        log.push(rec("B"));
        let b = log.take_undo().unwrap();
        log.undone(b);
        let a = log.take_undo().unwrap();
        log.undone(a);
        let a = log.take_redo().unwrap();
        log.return_redo(a);
        assert_eq!(log.redo_label(), Some("A"));
        assert_eq!(log.undo_label(), None);
    }

    #[test]
    fn invert_swaps_pairs() {
        let p = vec![("a".to_string(), "b".to_string()), ("c".to_string(), "d".to_string())];
        assert_eq!(
            invert(&p),
            vec![("b".to_string(), "a".to_string()), ("d".to_string(), "c".to_string())]
        );
    }
}
