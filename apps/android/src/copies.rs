//! The activity's copies of picked documents in the activity stack: the Java copy loop reports
//! through [`lcc_android_host::CopyProgress`]; each copy holds a [`TaskGuard`] (kind `import`)
//! from its first report until it ends, and the row's ✕ answers the loop's next report with
//! "stop".

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use lcc_engine::activity::{Activity, Cancel, TaskGuard};

/// The row's label (translated by the stack; in every `locales/fork/<code>.json`).
pub const LABEL: &str = "Copying picked files";

/// The copies in flight, by the activity's task number.
pub struct CopyRows {
    activity: Activity,
    rows: Mutex<HashMap<i64, TaskGuard>>,
}

impl CopyRows {
    pub fn new(activity: Activity) -> CopyRows {
        CopyRows { activity, rows: Mutex::new(HashMap::new()) }
    }
}

impl lcc_android_host::CopyProgress for CopyRows {
    fn progress(&self, task: i64, done: u64, total: u64, detail: &str) -> bool {
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        let guard = rows.entry(task).or_insert_with(|| self.activity.start("import", LABEL, Cancel::Yes));
        guard.progress(done, total);
        guard.detail(detail);
        guard.is_cancelled()
    }

    fn ended(&self, task: i64) {
        // dropping the guard removes the row
        let guard = self.rows.lock().unwrap_or_else(PoisonError::into_inner).remove(&task);
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    use lcc_android_host::CopyProgress;

    use super::*;

    #[test]
    fn a_copy_shows_a_row_until_it_ends_and_can_be_stopped() {
        let activity = Activity::default();
        let rows = CopyRows::new(activity.clone());
        assert!(!rows.progress(7, 1, 0, "IMG_1.CR3"));
        let list = activity.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "import");
        assert!(list[0].cancellable);
        // a second report updates the same row
        assert!(!rows.progress(7, 2, 5, "IMG_2.CR3"));
        assert_eq!(activity.list().len(), 1);
        activity.cancel(list[0].id).unwrap();
        assert!(rows.progress(7, 3, 5, "IMG_3.CR3"), "the copy loop is told to stop");
        rows.ended(7);
        assert!(activity.list().is_empty());
        // an end for a copy that never reported is harmless
        rows.ended(8);
    }
}
