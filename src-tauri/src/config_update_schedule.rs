use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use camellia_nexus_core::ProgramId;

const FAILURE_RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(crate) struct ScheduleBook {
    entries: HashMap<ProgramId, ScheduleEntry>,
}

struct ScheduleEntry {
    interval: Duration,
    next: Instant,
    last_completed: Option<Instant>,
}

impl ScheduleBook {
    pub(crate) fn due(
        &mut self,
        policies: Vec<(ProgramId, Duration, Option<Instant>)>,
        now: Instant,
    ) -> Vec<ProgramId> {
        let active: HashSet<_> = policies.iter().map(|(id, _, _)| id.clone()).collect();
        self.entries.retain(|id, _| active.contains(id));
        let mut due = Vec::new();
        for (id, interval, last_completed) in policies {
            let entry = self
                .entries
                .entry(id.clone())
                .or_insert_with(|| ScheduleEntry {
                    interval,
                    next: last_completed.unwrap_or(now) + interval,
                    last_completed,
                });
            if entry.interval != interval {
                entry.interval = interval;
                entry.next = last_completed.unwrap_or(now) + interval;
                entry.last_completed = last_completed;
            } else if last_completed > entry.last_completed {
                entry.last_completed = last_completed;
                entry.next = last_completed.unwrap_or(now) + interval;
            } else if now >= entry.next {
                entry.next = now + interval;
                due.push(id);
            }
        }
        due
    }

    pub(crate) fn retry_soon(&mut self, id: &ProgramId, now: Instant) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.next = now + entry.interval.min(FAILURE_RETRY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ProgramId {
        ProgramId::parse(value).expect("program id")
    }

    #[test]
    fn schedule_starts_after_the_interval_and_resets_when_changed() {
        let start = Instant::now();
        let mut schedule = ScheduleBook::default();
        assert!(
            schedule
                .due(vec![(id("alpha"), Duration::from_secs(60), None)], start)
                .is_empty()
        );
        assert_eq!(
            schedule.due(
                vec![(id("alpha"), Duration::from_secs(60), None)],
                start + Duration::from_secs(60),
            ),
            vec![id("alpha")],
        );
        assert!(
            schedule
                .due(
                    vec![(id("alpha"), Duration::from_secs(120), None)],
                    start + Duration::from_secs(61),
                )
                .is_empty()
        );
        assert!(
            schedule
                .due(Vec::new(), start + Duration::from_secs(500))
                .is_empty()
        );
        assert!(schedule.entries.is_empty());
    }

    #[test]
    fn completed_manual_update_resets_the_automatic_deadline() {
        let start = Instant::now();
        let completed = start + Duration::from_secs(50);
        let mut schedule = ScheduleBook::default();
        schedule.due(vec![(id("alpha"), Duration::from_secs(60), None)], start);
        assert!(
            schedule
                .due(
                    vec![(id("alpha"), Duration::from_secs(60), Some(completed))],
                    start + Duration::from_secs(60),
                )
                .is_empty()
        );
        assert_eq!(
            schedule.due(
                vec![(id("alpha"), Duration::from_secs(60), Some(completed))],
                completed + Duration::from_secs(60),
            ),
            vec![id("alpha")],
        );
    }

    #[test]
    fn failure_uses_the_shorter_retry_window() {
        let start = Instant::now();
        let program_id = id("alpha");
        let mut schedule = ScheduleBook::default();
        schedule.due(
            vec![(program_id.clone(), Duration::from_secs(3600), None)],
            start,
        );
        schedule.retry_soon(&program_id, start + Duration::from_secs(10));
        assert_eq!(
            schedule.due(
                vec![(program_id.clone(), Duration::from_secs(3600), None)],
                start + Duration::from_secs(310),
            ),
            vec![program_id],
        );
    }
}
