use anyhow::{Context as _, Result};
use buffer_diff::BufferDiff;
use collections::{BTreeMap, HashMap, HashSet};
use gpui::{App, AppContext, AsyncApp, Context, Entity, Subscription, Task, WeakEntity};
use language::{
    Buffer, BufferEvent, DiskState, OffsetRangeExt, Operation, TextBufferSnapshot, ToOffset,
};
use std::{ops::Range, sync::Arc};

/// Tracks actions performed by tools in a thread
pub struct ActionLog {
    /// Buffers that user manually added to the context, and whose content has
    /// changed since the model last saw them.
    stale_buffers_in_context: HashSet<Entity<Buffer>>,
    /// Buffers that we want to notify the model about when they change.
    tracked_buffers: BTreeMap<Entity<Buffer>, TrackedBuffer>,
    /// Has the model edited a file since it last checked diagnostics?
    edited_since_project_diagnostics_check: bool,
}

impl ActionLog {
    /// Creates a new, empty action log.
    pub fn new() -> Self {
        Self {
            stale_buffers_in_context: HashSet::default(),
            tracked_buffers: BTreeMap::default(),
            edited_since_project_diagnostics_check: false,
        }
    }

    /// Notifies a diagnostics check
    pub fn checked_project_diagnostics(&mut self) {
        self.edited_since_project_diagnostics_check = false;
    }

    /// Returns true if any files have been edited since the last project diagnostics check
    pub fn has_edited_files_since_project_diagnostics_check(&self) -> bool {
        self.edited_since_project_diagnostics_check
    }

    fn track_buffer(
        &mut self,
        buffer: Entity<Buffer>,
        created: bool,
        cx: &mut Context<Self>,
    ) -> &mut TrackedBuffer {
        let tracked_buffer = self
            .tracked_buffers
            .entry(buffer.clone())
            .or_insert_with(|| {
                let text_snapshot = buffer.read(cx).text_snapshot();
                let unreviewed_diff = cx.new(|cx| BufferDiff::new(&text_snapshot, cx));
                let diff = cx.new(|cx| {
                    let mut diff = BufferDiff::new(&text_snapshot, cx);
                    diff.set_secondary_diff(unreviewed_diff.clone());
                    diff
                });
                let (diff_update_tx, diff_update_rx) = async_watch::channel(());
                TrackedBuffer {
                    buffer: buffer.clone(),
                    change: Change::Edited {
                        edit_ids: HashSet::default(),
                        initial_content: if created {
                            None
                        } else {
                            Some(text_snapshot.clone())
                        },
                    },
                    version: buffer.read(cx).version(),
                    diff,
                    diff_update: diff_update_tx,
                    _maintain_diff: cx.spawn({
                        let buffer = buffer.clone();
                        async move |this, cx| {
                            Self::maintain_diff(this, buffer, diff_update_rx, cx)
                                .await
                                .ok();
                        }
                    }),
                    _subscription: cx.subscribe(&buffer, Self::handle_buffer_event),
                }
            });
        tracked_buffer.version = buffer.read(cx).version();
        tracked_buffer
    }

    fn handle_buffer_event(
        &mut self,
        buffer: Entity<Buffer>,
        event: &BufferEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BufferEvent::Operation { operation, .. } => {
                self.handle_buffer_operation(buffer, operation, cx)
            }
            BufferEvent::FileHandleChanged => {
                self.handle_buffer_file_changed(buffer, cx);
            }
            _ => {}
        };
    }

    fn handle_buffer_operation(
        &mut self,
        buffer: Entity<Buffer>,
        operation: &Operation,
        cx: &mut Context<Self>,
    ) {
        let Some(tracked_buffer) = self.tracked_buffers.get_mut(&buffer) else {
            return;
        };
        let Operation::Buffer(text::Operation::Edit(operation)) = operation else {
            return;
        };
        let Change::Edited { edit_ids, .. } = &mut tracked_buffer.change else {
            return;
        };
        if edit_ids.contains(&operation.timestamp) {
            return;
        }

        // If the buffer operation overlaps with any tracked edits, mark it as unreviewed.
        let buffer = buffer.read(cx);
        let operation_edit_ranges = buffer
            .edited_ranges_for_edit_ids::<usize>([&operation.timestamp])
            .collect::<Vec<_>>();
        let tracked_edit_ranges = buffer.edited_ranges_for_edit_ids::<usize>(edit_ids.iter());
        if ranges_intersect(operation_edit_ranges, tracked_edit_ranges) {
            edit_ids.insert(operation.timestamp);
            tracked_buffer.schedule_diff_update();
        }
    }

    fn handle_buffer_file_changed(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        let Some(tracked_buffer) = self.tracked_buffers.get_mut(&buffer) else {
            return;
        };

        match tracked_buffer.change {
            Change::Deleted { .. } => {
                if buffer
                    .read(cx)
                    .file()
                    .map_or(false, |file| file.disk_state() != DiskState::Deleted)
                {
                    // If the buffer had been deleted by a tool, but it got
                    // resurrected externally, we want to clear the changes we
                    // were tracking and reset the buffer's state.
                    tracked_buffer.change = Change::Edited {
                        edit_ids: HashSet::default(),
                        initial_content: Some(buffer.read(cx).text_snapshot()),
                    };
                }
                tracked_buffer.schedule_diff_update();
            }
            Change::Edited { .. } => {
                if buffer
                    .read(cx)
                    .file()
                    .map_or(false, |file| file.disk_state() == DiskState::Deleted)
                {
                    // If the buffer had been edited by a tool, but it got
                    // deleted externally, we want to stop tracking it.
                    self.tracked_buffers.remove(&buffer);
                } else {
                    tracked_buffer.schedule_diff_update();
                }
            }
        }
    }

    async fn maintain_diff(
        this: WeakEntity<Self>,
        buffer: Entity<Buffer>,
        mut diff_update: async_watch::Receiver<()>,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        while let Some(_) = diff_update.recv().await.ok() {
            let update = this.update(cx, |this, cx| {
                let tracked_buffer = this
                    .tracked_buffers
                    .get_mut(&buffer)
                    .context("buffer not tracked")?;
                anyhow::Ok(tracked_buffer.update_diff(cx))
            })??;
            update.await;
            this.update(cx, |_this, cx| cx.notify())?;
        }

        Ok(())
    }

    /// Track a buffer as read, so we can notify the model about user edits.
    pub fn buffer_read(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        self.track_buffer(buffer, false, cx);
    }

    /// Track a buffer as read, so we can notify the model about user edits.
    pub fn will_create_buffer(
        &mut self,
        buffer: Entity<Buffer>,
        edit_id: Option<clock::Lamport>,
        cx: &mut Context<Self>,
    ) {
        self.track_buffer(buffer.clone(), true, cx);
        self.buffer_edited(buffer, edit_id.into_iter().collect(), cx)
    }

    /// Mark a buffer as edited, so we can refresh it in the context
    pub fn buffer_edited(
        &mut self,
        buffer: Entity<Buffer>,
        mut edit_ids: Vec<clock::Lamport>,
        cx: &mut Context<Self>,
    ) {
        self.edited_since_project_diagnostics_check = true;
        self.stale_buffers_in_context.insert(buffer.clone());

        let tracked_buffer = self.track_buffer(buffer.clone(), false, cx);

        match &mut tracked_buffer.change {
            Change::Edited {
                edit_ids: existing_edit_ids,
                ..
            } => {
                existing_edit_ids.extend(edit_ids);
            }
            Change::Deleted {
                deleted_content,
                deletion_id,
                ..
            } => {
                edit_ids.extend(*deletion_id);
                tracked_buffer.change = Change::Edited {
                    edit_ids: edit_ids.into_iter().collect(),
                    initial_content: Some(deleted_content.clone()),
                };
            }
        }

        tracked_buffer.schedule_diff_update();
    }

    pub fn will_delete_buffer(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        let tracked_buffer = self.track_buffer(buffer.clone(), false, cx);
        if let Change::Edited {
            initial_content, ..
        } = &tracked_buffer.change
        {
            if let Some(initial_content) = initial_content {
                let deletion_id = buffer.update(cx, |buffer, cx| buffer.set_text("", cx));
                tracked_buffer.change = Change::Deleted {
                    deleted_content: initial_content.clone(),
                    deletion_id,
                };
                tracked_buffer.schedule_diff_update();
            } else {
                self.tracked_buffers.remove(&buffer);
                cx.notify();
            }
        }
    }

    pub fn keep_edits_in_range<T: ToOffset>(
        &mut self,
        buffer_handle: Entity<Buffer>,
        buffer_range: Range<T>,
        cx: &mut Context<Self>,
    ) {
        let Some(tracked_buffer) = self.tracked_buffers.get_mut(&buffer_handle) else {
            return;
        };

        let buffer = buffer_handle.read(cx);
        let buffer_range = buffer_range.to_offset(buffer);

        match &mut tracked_buffer.change {
            Change::Deleted { .. } => {
                self.tracked_buffers.remove(&buffer_handle);
                cx.notify();
            }
            Change::Edited { edit_ids, .. } => {
                edit_ids.retain(|edit_id| {
                    for range in buffer.edited_ranges_for_edit_ids::<usize>([edit_id]) {
                        if buffer_range.end >= range.start && buffer_range.start <= range.end {
                            return false;
                        }
                    }
                    true
                });
                tracked_buffer.schedule_diff_update();
            }
        }
    }

    pub fn keep_all_edits(&mut self) {
        todo!();
    }

    /// Returns the set of buffers that contain changes that haven't been reviewed by the user.
    pub fn changed_buffers(&self, cx: &App) -> BTreeMap<Entity<Buffer>, Entity<BufferDiff>> {
        self.tracked_buffers
            .iter()
            .filter(|(_, tracked)| tracked.has_changes(cx))
            .map(|(buffer, tracked)| (buffer.clone(), tracked.diff.clone()))
            .collect()
    }

    /// Iterate over buffers changed since last read or edited by the model
    pub fn stale_buffers<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = &'a Entity<Buffer>> {
        self.tracked_buffers
            .iter()
            .filter(|(buffer, tracked)| tracked.version != buffer.read(cx).version)
            .map(|(buffer, _)| buffer)
    }

    /// Takes and returns the set of buffers pending refresh, clearing internal state.
    pub fn take_stale_buffers_in_context(&mut self) -> HashSet<Entity<Buffer>> {
        std::mem::take(&mut self.stale_buffers_in_context)
    }
}

fn ranges_intersect(
    ranges_a: impl IntoIterator<Item = Range<usize>>,
    ranges_b: impl IntoIterator<Item = Range<usize>>,
) -> bool {
    let mut ranges_a_iter = ranges_a.into_iter().peekable();
    let mut ranges_b_iter = ranges_b.into_iter().peekable();
    while let (Some(range_a), Some(range_b)) = (ranges_a_iter.peek(), ranges_b_iter.peek()) {
        if range_a.end < range_b.start {
            ranges_a_iter.next();
        } else if range_b.end < range_a.start {
            ranges_b_iter.next();
        } else {
            return true;
        }
    }
    false
}

struct TrackedBuffer {
    buffer: Entity<Buffer>,
    change: Change,
    version: clock::Global,
    diff: Entity<BufferDiff>,
    diff_update: async_watch::Sender<()>,
    _maintain_diff: Task<()>,
    _subscription: Subscription,
}

enum Change {
    Edited {
        edit_ids: HashSet<clock::Lamport>,
        initial_content: Option<TextBufferSnapshot>,
    },
    Deleted {
        deleted_content: TextBufferSnapshot,
        deletion_id: Option<clock::Lamport>,
    },
}

impl TrackedBuffer {
    fn has_changes(&self, cx: &App) -> bool {
        self.diff
            .read(cx)
            .hunks(&self.buffer.read(cx), cx)
            .next()
            .is_some()
    }

    fn schedule_diff_update(&self) {
        self.diff_update.send(()).ok();
    }

    fn update_diff(&mut self, cx: &mut App) -> Task<()> {
        match &self.change {
            Change::Edited { edit_ids, .. } => {
                let edits_to_undo = edit_ids
                    .iter()
                    .map(|edit_id| (*edit_id, u32::MAX))
                    .collect::<HashMap<_, _>>();
                let buffer_without_edits = self.buffer.update(cx, |buffer, cx| buffer.branch(cx));
                buffer_without_edits
                    .update(cx, |buffer, cx| buffer.undo_operations(edits_to_undo, cx));
                let diff_update = self.diff.update(cx, |diff, cx| {
                    diff.set_base_text(
                        buffer_without_edits,
                        self.buffer.read(cx).text_snapshot(),
                        cx,
                    )
                });

                cx.background_spawn(async move {
                    _ = diff_update.await;
                })
            }
            Change::Deleted {
                deleted_content, ..
            } => {
                let deleted_content = deleted_content.clone();

                let diff = self.diff.clone();
                let buffer_snapshot = self.buffer.read(cx).text_snapshot();
                let language = self.buffer.read(cx).language().cloned();
                let language_registry = self.buffer.read(cx).language_registry().clone();

                cx.spawn(async move |cx| {
                    let base_text = Arc::new(deleted_content.text());

                    let diff_snapshot = BufferDiff::update_diff(
                        diff.clone(),
                        buffer_snapshot.clone(),
                        Some(base_text.clone()),
                        true,
                        false,
                        language.clone(),
                        language_registry.clone(),
                        cx,
                    )
                    .await;
                    if let Ok(diff_snapshot) = diff_snapshot {
                        diff.update(cx, |diff, cx| {
                            diff.set_snapshot(&buffer_snapshot, diff_snapshot, false, None, cx)
                        })
                        .ok();
                    }
                })
            }
        }
    }
}

pub struct ChangedBuffer {
    pub diff: Entity<BufferDiff>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffer_diff::DiffHunkStatusKind;
    use gpui::TestAppContext;
    use language::Point;
    use project::{FakeFs, Fs, Project, RemoveOptions};
    use serde_json::json;
    use settings::SettingsStore;
    use util::path;

    #[gpui::test(iterations = 10)]
    async fn test_edit_review(cx: &mut TestAppContext) {
        let action_log = cx.new(|_| ActionLog::new());
        let buffer = cx.new(|cx| Buffer::local("abc\ndef\nghi\njkl\nmno", cx));

        let edit1 = buffer.update(cx, |buffer, cx| {
            buffer
                .edit([(Point::new(1, 1)..Point::new(1, 2), "E")], None, cx)
                .unwrap()
        });
        let edit2 = buffer.update(cx, |buffer, cx| {
            buffer
                .edit([(Point::new(4, 2)..Point::new(4, 3), "O")], None, cx)
                .unwrap()
        });
        assert_eq!(
            buffer.read_with(cx, |buffer, _| buffer.text()),
            "abc\ndEf\nghi\njkl\nmnO"
        );

        action_log.update(cx, |log, cx| {
            log.buffer_edited(buffer.clone(), vec![edit1, edit2], cx)
        });
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(
                buffer.clone(),
                vec![
                    HunkStatus {
                        range: Point::new(1, 0)..Point::new(2, 0),
                        diff_status: DiffHunkStatusKind::Modified,
                        old_text: "def\n".into(),
                    },
                    HunkStatus {
                        range: Point::new(4, 0)..Point::new(4, 3),
                        diff_status: DiffHunkStatusKind::Modified,
                        old_text: "mno".into(),
                    }
                ],
            )]
        );

        action_log.update(cx, |log, cx| {
            log.keep_edits_in_range(buffer.clone(), Point::new(3, 0)..Point::new(4, 3), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(
                buffer.clone(),
                vec![HunkStatus {
                    range: Point::new(1, 0)..Point::new(2, 0),
                    diff_status: DiffHunkStatusKind::Modified,
                    old_text: "def\n".into(),
                }],
            )]
        );

        action_log.update(cx, |log, cx| {
            log.keep_edits_in_range(buffer.clone(), Point::new(0, 0)..Point::new(4, 3), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(buffer.clone(), vec![])]
        );
    }

    #[gpui::test(iterations = 10)]
    async fn test_overlapping_user_edits(cx: &mut TestAppContext) {
        let action_log = cx.new(|_| ActionLog::new());
        let buffer = cx.new(|cx| Buffer::local("abc\ndef\nghi\njkl\nmno", cx));

        let tool_edit = buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    [(Point::new(0, 2)..Point::new(2, 3), "C\nDEF\nGHI")],
                    None,
                    cx,
                )
                .unwrap()
        });
        assert_eq!(
            buffer.read_with(cx, |buffer, _| buffer.text()),
            "abC\nDEF\nGHI\njkl\nmno"
        );

        action_log.update(cx, |log, cx| {
            log.buffer_edited(buffer.clone(), vec![tool_edit], cx)
        });
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(
                buffer.clone(),
                vec![HunkStatus {
                    range: Point::new(0, 0)..Point::new(3, 0),
                    diff_status: DiffHunkStatusKind::Modified,
                    old_text: "abc\ndef\nghi\n".into(),
                }],
            )]
        );

        buffer.update(cx, |buffer, cx| {
            buffer.edit([(Point::new(0, 2)..Point::new(0, 2), "X")], None, cx)
        });
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(
                buffer.clone(),
                vec![HunkStatus {
                    range: Point::new(0, 0)..Point::new(3, 0),
                    diff_status: DiffHunkStatusKind::Modified,
                    old_text: "abXc\ndef\nghi\n".into(),
                }],
            )]
        );

        action_log.update(cx, |log, cx| {
            log.keep_edits_in_range(buffer.clone(), Point::new(0, 0)..Point::new(1, 0), cx)
        });
        cx.run_until_parked();
        assert_eq!(unreviewed_hunks(&action_log, cx), vec![]);
    }

    #[gpui::test(iterations = 10)]
    async fn test_deletion(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            language::init(cx);
            Project::init_settings(cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/dir"),
            json!({"file1": "lorem\n", "file2": "ipsum\n"}),
        )
        .await;

        let project = Project::test(fs.clone(), [path!("/dir").as_ref()], cx).await;
        let file1_path = project
            .read_with(cx, |project, cx| project.find_project_path("dir/file1", cx))
            .unwrap();
        let file2_path = project
            .read_with(cx, |project, cx| project.find_project_path("dir/file2", cx))
            .unwrap();

        let action_log = cx.new(|_| ActionLog::new());
        let buffer1 = project
            .update(cx, |project, cx| {
                project.open_buffer(file1_path.clone(), cx)
            })
            .await
            .unwrap();
        let buffer2 = project
            .update(cx, |project, cx| {
                project.open_buffer(file2_path.clone(), cx)
            })
            .await
            .unwrap();

        action_log.update(cx, |log, cx| log.will_delete_buffer(buffer1.clone(), cx));
        action_log.update(cx, |log, cx| log.will_delete_buffer(buffer2.clone(), cx));
        project
            .update(cx, |project, cx| {
                project.delete_file(file1_path.clone(), false, cx)
            })
            .unwrap()
            .await
            .unwrap();
        project
            .update(cx, |project, cx| {
                project.delete_file(file2_path.clone(), false, cx)
            })
            .unwrap()
            .await
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![
                (
                    buffer1.clone(),
                    vec![HunkStatus {
                        range: Point::new(0, 0)..Point::new(0, 0),
                        diff_status: DiffHunkStatusKind::Deleted,
                        old_text: "lorem\n".into(),
                    }]
                ),
                (
                    buffer2.clone(),
                    vec![HunkStatus {
                        range: Point::new(0, 0)..Point::new(0, 0),
                        diff_status: DiffHunkStatusKind::Deleted,
                        old_text: "ipsum\n".into(),
                    }],
                )
            ]
        );

        // Simulate file1 being recreated externally.
        fs.insert_file(path!("/dir/file1"), "LOREM".as_bytes().to_vec())
            .await;
        let buffer2 = project
            .update(cx, |project, cx| project.open_buffer(file2_path, cx))
            .await
            .unwrap();
        cx.run_until_parked();
        // Simulate file2 being recreated by a tool.
        let edit_id = buffer2.update(cx, |buffer, cx| buffer.set_text("IPSUM", cx));
        action_log.update(cx, |log, cx| {
            log.will_create_buffer(buffer2.clone(), edit_id, cx)
        });
        project
            .update(cx, |project, cx| project.save_buffer(buffer2.clone(), cx))
            .await
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            unreviewed_hunks(&action_log, cx),
            vec![(
                buffer2.clone(),
                vec![HunkStatus {
                    range: Point::new(0, 0)..Point::new(0, 5),
                    diff_status: DiffHunkStatusKind::Modified,
                    old_text: "ipsum\n".into(),
                }],
            )]
        );

        // Simulate file2 being deleted externally.
        fs.remove_file(path!("/dir/file2").as_ref(), RemoveOptions::default())
            .await
            .unwrap();
        cx.run_until_parked();
        assert_eq!(unreviewed_hunks(&action_log, cx), vec![]);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct HunkStatus {
        range: Range<Point>,
        diff_status: DiffHunkStatusKind,
        old_text: String,
    }

    fn unreviewed_hunks(
        action_log: &Entity<ActionLog>,
        cx: &TestAppContext,
    ) -> Vec<(Entity<Buffer>, Vec<HunkStatus>)> {
        cx.read(|cx| {
            action_log
                .read(cx)
                .changed_buffers(cx)
                .into_iter()
                .map(|(buffer, diff)| {
                    let snapshot = buffer.read(cx).snapshot();
                    (
                        buffer,
                        diff.read(cx)
                            .hunks(&snapshot, cx)
                            .map(|hunk| HunkStatus {
                                diff_status: hunk.status().kind,
                                range: hunk.range,
                                old_text: diff
                                    .read(cx)
                                    .base_text()
                                    .text_for_range(hunk.diff_base_byte_range)
                                    .collect(),
                            })
                            .collect(),
                    )
                })
                .collect()
        })
    }
}
