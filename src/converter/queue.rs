use super::JobSpec;
use std::{collections::VecDeque, path::Path};

pub(super) const MAX_PENDING_JOBS: usize = 32;

pub(super) struct PendingJobs {
    generation: u64,
    jobs: VecDeque<JobSpec>,
}

impl PendingJobs {
    pub(super) fn new(generation: u64) -> Self {
        Self {
            generation,
            jobs: VecDeque::new(),
        }
    }
    pub(super) fn len(&self) -> usize {
        self.jobs.len()
    }
    pub(super) fn contains_output(&self, path: &Path) -> bool {
        self.jobs.iter().any(|job| job.output == path)
    }
    pub(super) fn synchronize(&mut self, generation: u64) -> bool {
        if generation == self.generation {
            return false;
        }
        self.generation = generation;
        self.jobs.clear();
        true
    }
    pub(super) fn push(&mut self, job: JobSpec) -> Result<(), JobSpec> {
        if self.jobs.len() >= MAX_PENDING_JOBS {
            return Err(job);
        }
        self.jobs.push_back(job);
        Ok(())
    }
    pub(super) fn pop(&mut self) -> Option<JobSpec> {
        self.jobs.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::{
        COMMAND_CAPACITY, ConversionOptions, ConversionPreset, ConverterCommand, ConverterControl,
        submit,
    };
    use crossbeam_channel::bounded;
    use std::sync::atomic::Ordering;

    fn job(index: usize) -> JobSpec {
        JobSpec {
            source: format!("source-{index}.mp4").into(),
            output: format!("out-{index}.mp4").into(),
            preset: ConversionPreset::Mp4H264,
            options: ConversionOptions::default(),
            duration_seconds: 10.0,
        }
    }
    fn request(control: &ConverterControl, index: usize) -> ConverterCommand {
        let job = job(index);
        ConverterCommand::Start {
            queue_generation: control.queue_generation(),
            source: job.source,
            output: job.output,
            preset: job.preset,
            options: job.options,
            duration_seconds: job.duration_seconds,
        }
    }

    #[test]
    fn pending_jobs_have_a_hard_limit_and_keep_fifo_order() {
        let mut queue = PendingJobs::new(0);
        for index in 0..MAX_PENDING_JOBS {
            assert!(queue.push(job(index)).is_ok());
        }
        assert!(queue.push(job(99)).is_err());
        assert_eq!(queue.len(), MAX_PENDING_JOBS);
        for index in 0..MAX_PENDING_JOBS {
            assert_eq!(queue.pop().unwrap().output, job(index).output);
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn full_command_queue_cannot_block_cancel_clear_or_submitter() {
        let (tx, rx) = bounded(COMMAND_CAPACITY);
        let control = ConverterControl::default();
        for index in 0..COMMAND_CAPACITY {
            submit(&tx, request(&control, index)).unwrap();
        }
        assert!(
            submit(&tx, request(&control, 99))
                .unwrap_err()
                .contains("не добавлена")
        );
        control.cancel_active();
        control.clear_pending();
        assert!(control.cancel.load(Ordering::Acquire));
        assert_eq!(control.queue_generation(), 1);
        assert_eq!(rx.len(), COMMAND_CAPACITY);
        drop(rx);
        assert!(
            submit(&tx, request(&control, 99))
                .unwrap_err()
                .contains("недоступен")
        );
    }

    #[test]
    fn clear_discards_old_requests_but_preserves_requests_submitted_after_it() {
        let control = ConverterControl::default();
        let old = request(&control, 0);
        let mut queue = PendingJobs::new(control.queue_generation());
        assert!(queue.push(job(0)).is_ok());
        control.clear_pending();
        let new = request(&control, 1);
        assert!(queue.synchronize(control.queue_generation()));
        assert_eq!(queue.len(), 0);
        let ConverterCommand::Start {
            queue_generation: old_generation,
            ..
        } = old
        else {
            panic!()
        };
        let ConverterCommand::Start {
            queue_generation: new_generation,
            ..
        } = new
        else {
            panic!()
        };
        assert_ne!(old_generation, control.queue_generation());
        assert_eq!(new_generation, control.queue_generation());
        assert!(queue.push(job(1)).is_ok());
        assert!(!queue.synchronize(control.queue_generation()));
        assert!(queue.contains_output(&job(1).output));
        control.clear_pending();
        assert!(queue.synchronize(control.queue_generation()));
        assert!(!queue.contains_output(&job(1).output));
    }

    #[test]
    fn repeated_availability_refreshes_share_one_request_and_retry_after_full() {
        let (tx, rx) = bounded(1);
        let control = ConverterControl::default();
        for _ in 0..1000 {
            control.request_refresh(&tx).unwrap();
        }
        assert_eq!(rx.len(), 1);
        control.refresh_pending.store(false, Ordering::Release); // dispatcher completed probe
        assert!(control.request_refresh(&tx).is_err());
        assert!(!control.refresh_pending.load(Ordering::Acquire));
        rx.recv().unwrap();
        control.request_refresh(&tx).unwrap();
        assert_eq!(rx.len(), 1);
    }
}
