//! Motion session tracking.
//!
//! The raw detector tells us whether each sampled frame looks active. This module groups those
//! frame-level signals into longer motion sessions so we can choose better representative snapshots.

use std::time::SystemTime;

use crate::{
    config::MotionDetectionConfig,
    ffmpeg::VideoFrame,
    motion::{MotionAnalysis, MotionEvent},
};

pub struct MotionSessionTracker {
    local_confirmation_frames: usize,
    motion_end_grace_frames: u64,
    initial_capture_delay_frames: u64,
    periodic_capture_interval_frames: u64,
    session_cooldown_frames: u64,
    pending_local_frames: Vec<AnnotatedFrame>,
    active_session: Option<ActiveMotionSession>,
    next_session_allowed_frame: Option<u64>,
    next_session_id: u64,
}

impl MotionSessionTracker {
    pub fn new(settings: &MotionDetectionConfig) -> Self {
        let frame_rate = u64::from(settings.frame_rate);

        Self {
            local_confirmation_frames: usize::try_from(settings.local_motion_consecutive_frames)
                .unwrap_or(usize::MAX),
            motion_end_grace_frames: settings.motion_end_grace_seconds.saturating_mul(frame_rate),
            initial_capture_delay_frames: settings
                .motion_snapshot_delay_seconds
                .saturating_mul(frame_rate),
            periodic_capture_interval_frames: settings
                .long_motion_snapshot_interval_seconds
                .saturating_mul(frame_rate),
            session_cooldown_frames: settings.event_cooldown_seconds.saturating_mul(frame_rate),
            pending_local_frames: Vec::new(),
            active_session: None,
            next_session_allowed_frame: None,
            next_session_id: 1,
        }
    }

    pub fn ingest(
        &mut self,
        frame: VideoFrame,
        analysis: MotionAnalysis,
    ) -> Vec<MotionSessionCapture> {
        self.ingest_internal(frame, analysis).captures
    }

    pub fn finish(&mut self) -> Vec<MotionSessionCapture> {
        self.finish_internal().captures
    }

    pub fn ingest_events(
        &mut self,
        frame: VideoFrame,
        analysis: MotionAnalysis,
    ) -> Vec<MotionSessionEvent> {
        self.ingest_internal(frame, analysis).events
    }

    pub fn finish_events(&mut self) -> Vec<MotionSessionEvent> {
        self.finish_internal().events
    }

    fn ingest_internal(&mut self, frame: VideoFrame, analysis: MotionAnalysis) -> TrackerOutput {
        let annotated = AnnotatedFrame { frame, analysis };

        if let Some(mut session) = self.active_session.take() {
            if annotated.analysis.is_motion_active() {
                let events = session.ingest_active_frame(
                    annotated,
                    self.initial_capture_delay_frames,
                    self.periodic_capture_interval_frames,
                );
                self.active_session = Some(session);
                return TrackerOutput::from_events(events);
            }

            session.inactive_frames = session.inactive_frames.saturating_add(1);
            if session.inactive_frames >= self.motion_end_grace_frames {
                return self.finalize_session(session);
            }

            self.active_session = Some(session);
            return TrackerOutput::default();
        }

        if self.is_in_cooldown(annotated.frame.index) {
            self.pending_local_frames.clear();
            return TrackerOutput::default();
        }

        if annotated.analysis.global_triggered {
            self.pending_local_frames.clear();
            self.start_session(vec![annotated]);
            return TrackerOutput::default();
        }

        if annotated.analysis.local_triggered {
            self.pending_local_frames.push(annotated);
            if self.pending_local_frames.len() >= self.local_confirmation_frames {
                let confirmed_frames = std::mem::take(&mut self.pending_local_frames);
                self.start_session(confirmed_frames);
            }
        } else {
            self.pending_local_frames.clear();
        }

        TrackerOutput::default()
    }

    fn finish_internal(&mut self) -> TrackerOutput {
        self.pending_local_frames.clear();

        match self.active_session.take() {
            Some(session) => self.finalize_session(session),
            None => TrackerOutput::default(),
        }
    }

    fn is_in_cooldown(&self, frame_index: u64) -> bool {
        match self.next_session_allowed_frame {
            Some(next_allowed_frame) => frame_index < next_allowed_frame,
            None => false,
        }
    }

    fn start_session(&mut self, frames: Vec<AnnotatedFrame>) {
        let session_id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);

        let mut session = ActiveMotionSession::new(session_id);
        for frame in frames {
            let _ = session.ingest_active_frame(
                frame,
                self.initial_capture_delay_frames,
                self.periodic_capture_interval_frames,
            );
        }

        self.active_session = Some(session);
    }

    fn finalize_session(&mut self, mut session: ActiveMotionSession) -> TrackerOutput {
        let output = session.finalize();
        self.next_session_allowed_frame = Some(
            session
                .last_active_frame_index
                .saturating_add(self.session_cooldown_frames),
        );
        output
    }
}

#[derive(Clone)]
struct AnnotatedFrame {
    frame: VideoFrame,
    analysis: MotionAnalysis,
}

struct ActiveMotionSession {
    session_id: u64,
    started_at: Option<SystemTime>,
    start_frame_index: u64,
    last_active_at: Option<SystemTime>,
    last_active_frame_index: u64,
    inactive_frames: u64,
    pre_capture_frames: Vec<AnnotatedFrame>,
    selected_frames: Vec<AnnotatedFrame>,
    initial_capture_selected: bool,
    next_periodic_target_frame: Option<u64>,
    previous_active_frame: Option<AnnotatedFrame>,
}

impl ActiveMotionSession {
    fn new(session_id: u64) -> Self {
        Self {
            session_id,
            started_at: None,
            start_frame_index: 0,
            last_active_at: None,
            last_active_frame_index: 0,
            inactive_frames: 0,
            pre_capture_frames: Vec::new(),
            selected_frames: Vec::new(),
            initial_capture_selected: false,
            next_periodic_target_frame: None,
            previous_active_frame: None,
        }
    }

    fn ingest_active_frame(
        &mut self,
        annotated: AnnotatedFrame,
        initial_capture_delay_frames: u64,
        periodic_capture_interval_frames: u64,
    ) -> Vec<MotionSessionEvent> {
        if self.started_at.is_none() {
            self.started_at = Some(annotated.frame.captured_at);
            self.start_frame_index = annotated.frame.index;
        }

        self.last_active_at = Some(annotated.frame.captured_at);
        self.last_active_frame_index = annotated.frame.index;
        self.inactive_frames = 0;

        let mut events = Vec::new();

        if !self.initial_capture_selected {
            self.pre_capture_frames.push(annotated.clone());
            let initial_target_frame = self
                .start_frame_index
                .saturating_add(initial_capture_delay_frames);

            if annotated.frame.index >= initial_target_frame {
                if let Some(selected) =
                    select_closest_frame(&self.pre_capture_frames, initial_target_frame)
                {
                    events.push(self.record_selected_frame(selected));
                }
                self.pre_capture_frames.clear();
                self.initial_capture_selected = true;
                self.next_periodic_target_frame = Some(
                    self.start_frame_index
                        .saturating_add(periodic_capture_interval_frames),
                );
            }

            self.previous_active_frame = Some(annotated);
            return events;
        }

        while let Some(target_frame_index) = self.next_periodic_target_frame {
            if annotated.frame.index < target_frame_index {
                break;
            }

            let selected = select_closest_from_pair(
                self.previous_active_frame.as_ref(),
                &annotated,
                target_frame_index,
            );
            events.push(self.record_selected_frame(selected));
            self.next_periodic_target_frame =
                Some(target_frame_index.saturating_add(periodic_capture_interval_frames));
        }

        self.previous_active_frame = Some(annotated);
        events
    }

    fn finalize(&mut self) -> TrackerOutput {
        let Some(motion_started_at) = self.started_at else {
            return TrackerOutput::default();
        };
        let Some(motion_ended_at) = self.last_active_at else {
            return TrackerOutput::default();
        };

        let mut events = Vec::new();
        if !self.initial_capture_selected
            && let Some(selected) = select_closest_frame(
                &self.pre_capture_frames,
                midpoint_frame(self.start_frame_index, self.last_active_frame_index),
            )
        {
            events.push(self.record_selected_frame(selected));
        }

        self.selected_frames.sort_by_key(|frame| frame.frame.index);
        self.selected_frames.dedup_by_key(|frame| frame.frame.index);

        let summary = MotionSessionSummary {
            session_id: self.session_id,
            motion_started_at,
            motion_started_frame_index: self.start_frame_index,
            motion_ended_at,
            motion_ended_frame_index: self.last_active_frame_index,
        };
        events.push(MotionSessionEvent::SessionFinished(summary));

        let captures = self
            .selected_frames
            .iter()
            .cloned()
            .map(|annotated| MotionSessionCapture {
                frame: annotated.frame,
                event: annotated.analysis.event(),
                motion_started_at,
                motion_started_frame_index: self.start_frame_index,
                motion_ended_at,
                motion_ended_frame_index: self.last_active_frame_index,
            })
            .collect();

        TrackerOutput { events, captures }
    }

    fn record_selected_frame(&mut self, selected: AnnotatedFrame) -> MotionSessionEvent {
        self.selected_frames.push(selected.clone());
        MotionSessionEvent::SnapshotSelected(MotionSnapshotSelection {
            session_id: self.session_id,
            frame: selected.frame,
            event: selected.analysis.event(),
        })
    }
}

fn midpoint_frame(start_frame_index: u64, end_frame_index: u64) -> u64 {
    start_frame_index.saturating_add(end_frame_index.saturating_sub(start_frame_index) / 2)
}

fn select_closest_frame(
    frames: &[AnnotatedFrame],
    target_frame_index: u64,
) -> Option<AnnotatedFrame> {
    frames
        .iter()
        .min_by_key(|frame| frame.frame.index.abs_diff(target_frame_index))
        .cloned()
}

fn select_closest_from_pair(
    previous: Option<&AnnotatedFrame>,
    current: &AnnotatedFrame,
    target_frame_index: u64,
) -> AnnotatedFrame {
    match previous {
        Some(previous) => {
            if previous.frame.index.abs_diff(target_frame_index)
                <= current.frame.index.abs_diff(target_frame_index)
            {
                previous.clone()
            } else {
                current.clone()
            }
        }
        None => current.clone(),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MotionSessionSummary {
    pub session_id: u64,
    pub motion_started_at: SystemTime,
    pub motion_started_frame_index: u64,
    pub motion_ended_at: SystemTime,
    pub motion_ended_frame_index: u64,
}

#[derive(Clone, Debug)]
pub struct MotionSnapshotSelection {
    pub session_id: u64,
    pub frame: VideoFrame,
    pub event: MotionEvent,
}

#[derive(Clone, Debug)]
pub enum MotionSessionEvent {
    SnapshotSelected(MotionSnapshotSelection),
    SessionFinished(MotionSessionSummary),
}

#[derive(Clone, Debug)]
pub struct MotionSessionCapture {
    pub frame: VideoFrame,
    pub event: MotionEvent,
    pub motion_started_at: SystemTime,
    pub motion_started_frame_index: u64,
    pub motion_ended_at: SystemTime,
    pub motion_ended_frame_index: u64,
}

#[derive(Default)]
struct TrackerOutput {
    events: Vec<MotionSessionEvent>,
    captures: Vec<MotionSessionCapture>,
}

impl TrackerOutput {
    fn from_events(events: Vec<MotionSessionEvent>) -> Self {
        Self {
            events,
            captures: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{MotionSessionCapture, MotionSessionEvent, MotionSessionTracker};
    use crate::{config::MotionDetectionConfig, ffmpeg::VideoFrame, motion::MotionAnalysis};

    fn build_frame(index: u64) -> VideoFrame {
        VideoFrame {
            index,
            captured_at: SystemTime::UNIX_EPOCH + Duration::from_secs(index),
            width: 2,
            height: 2,
            rgb: vec![0; 12],
        }
    }

    fn active_analysis() -> MotionAnalysis {
        MotionAnalysis {
            motion_ratio: 0.2,
            local_motion_ratio: 0.3,
            global_triggered: true,
            local_triggered: true,
        }
    }

    fn local_only_analysis() -> MotionAnalysis {
        MotionAnalysis {
            motion_ratio: 0.0,
            local_motion_ratio: 0.3,
            global_triggered: false,
            local_triggered: true,
        }
    }

    fn inactive_analysis() -> MotionAnalysis {
        MotionAnalysis {
            motion_ratio: 0.0,
            local_motion_ratio: 0.0,
            global_triggered: false,
            local_triggered: false,
        }
    }

    fn tracker_config() -> MotionDetectionConfig {
        MotionDetectionConfig {
            frame_rate: 1,
            motion_snapshot_delay_seconds: 5,
            long_motion_snapshot_interval_seconds: 30,
            motion_end_grace_seconds: 1,
            local_motion_consecutive_frames: 3,
            mqtt_host: String::new(),
            mqtt_topic: String::new(),
            output_directory: Some("/tmp/camwatch".into()),
            ..MotionDetectionConfig::default()
        }
    }

    fn capture_indexes(captures: &[MotionSessionCapture]) -> Vec<u64> {
        captures.iter().map(|capture| capture.frame.index).collect()
    }

    #[test]
    fn short_motion_uses_middle_frame() {
        let mut tracker = MotionSessionTracker::new(&tracker_config());

        assert!(tracker.ingest(build_frame(0), active_analysis()).is_empty());
        assert!(tracker.ingest(build_frame(1), active_analysis()).is_empty());
        assert!(tracker.ingest(build_frame(2), active_analysis()).is_empty());
        let captures = tracker.ingest(build_frame(3), inactive_analysis());

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].frame.index, 1);
        assert_eq!(captures[0].motion_started_frame_index, 0);
        assert_eq!(captures[0].motion_ended_frame_index, 2);
    }

    #[test]
    fn longer_motion_uses_five_second_frame() {
        let mut tracker = MotionSessionTracker::new(&tracker_config());

        for index in 0..8 {
            assert!(
                tracker
                    .ingest(build_frame(index), active_analysis())
                    .is_empty()
            );
        }

        let captures = tracker.ingest(build_frame(8), inactive_analysis());

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].frame.index, 5);
    }

    #[test]
    fn very_long_motion_adds_periodic_captures() {
        let mut tracker = MotionSessionTracker::new(&tracker_config());

        for index in 0..65 {
            assert!(
                tracker
                    .ingest(build_frame(index), active_analysis())
                    .is_empty()
            );
        }

        let captures = tracker.ingest(build_frame(65), inactive_analysis());

        assert_eq!(capture_indexes(&captures), vec![5, 30, 60]);
    }

    #[test]
    fn local_motion_confirmation_backdates_session_start() {
        let mut tracker = MotionSessionTracker::new(&tracker_config());

        assert!(
            tracker
                .ingest(build_frame(0), local_only_analysis())
                .is_empty()
        );
        assert!(
            tracker
                .ingest(build_frame(1), local_only_analysis())
                .is_empty()
        );
        assert!(
            tracker
                .ingest(build_frame(2), local_only_analysis())
                .is_empty()
        );
        let captures = tracker.ingest(build_frame(3), inactive_analysis());

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].motion_started_frame_index, 0);
        assert_eq!(captures[0].frame.index, 1);
    }

    #[test]
    fn event_api_emits_snapshot_selection_and_finish() {
        let mut tracker = MotionSessionTracker::new(&tracker_config());
        let mut saw_snapshot = false;

        for index in 0..8 {
            let events = tracker.ingest_events(build_frame(index), active_analysis());
            if index < 5 {
                assert!(events.is_empty());
            } else if index == 5 {
                assert_eq!(events.len(), 1);
                match &events[0] {
                    MotionSessionEvent::SnapshotSelected(selection) => {
                        assert_eq!(selection.frame.index, 5);
                        saw_snapshot = true;
                    }
                    MotionSessionEvent::SessionFinished(_) => {
                        panic!("expected snapshot selection at 5 seconds")
                    }
                }
            }
        }

        let events = tracker.ingest_events(build_frame(8), inactive_analysis());

        assert!(saw_snapshot);
        assert_eq!(events.len(), 1);
        match &events[0] {
            MotionSessionEvent::SessionFinished(summary) => {
                assert_eq!(summary.motion_started_frame_index, 0);
                assert_eq!(summary.motion_ended_frame_index, 7);
            }
            MotionSessionEvent::SnapshotSelected(_) => {
                panic!("expected only session finish on finalize")
            }
        }
    }
}
