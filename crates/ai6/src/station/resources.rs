//! The Station host's CPU and memory, sampled before each check-in.
//!
//! One sampler serves the whole process: every Station in it runs on the same host, and CPU
//! usage is the difference between two samples, so the sampler must outlive each check-in.

use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, System};

use crate::directorates::m::placement::Resources;

struct Sampler {
    system: System,
    /// When CPU usage was last refreshed; usage is meaningful only against an earlier refresh.
    cpu_refreshed: Option<Instant>,
}

impl Sampler {
    fn new() -> Self {
        Self {
            system: System::new(),
            cpu_refreshed: None,
        }
    }

    fn sample(&mut self) -> Resources {
        let previous = self.cpu_refreshed;
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let now = Instant::now();
        let cpu_percent = previous
            .filter(|previous| now.duration_since(*previous) >= MINIMUM_CPU_UPDATE_INTERVAL)
            .map(|_| f64::from(self.system.global_cpu_usage()));
        self.cpu_refreshed = Some(now);
        let cpus = self.system.cpus().len();
        let memory_total_mb = mebibytes(self.system.total_memory());
        Resources {
            cpus: (cpus > 0).then_some(cpus as f64),
            cpu_percent: cpu_percent.and_then(|percent| tenths(percent.clamp(0.0, 100.0))),
            // Windows reports zeros rather than having no load average.
            load_1m: cfg!(not(windows))
                .then(|| tenths(System::load_average().one))
                .flatten(),
            memory_total_mb,
            memory_available_mb: memory_total_mb
                .and_then(|_| mebibytes(self.system.available_memory())),
            sampled_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        }
    }
}

fn sampler() -> &'static Mutex<Sampler> {
    static SAMPLER: OnceLock<Mutex<Sampler>> = OnceLock::new();
    SAMPLER.get_or_init(|| Mutex::new(Sampler::new()))
}

/// Samples the host now. The first sample, and one taken sooner than sysinfo can measure
/// after the previous, has no CPU percent.
pub(crate) fn sample() -> Resources {
    match sampler().lock() {
        Ok(mut sampler) => sampler.sample(),
        Err(_) => Resources::default(),
    }
}

/// `value` rounded to one decimal, or `None` when it is not a finite number.
fn tenths(value: f64) -> Option<f64> {
    value.is_finite().then(|| (value * 10.0).round() / 10.0)
}

/// `bytes` in whole MiB, or `None` for zero, which sysinfo reports when it could not read it.
fn mebibytes(bytes: u64) -> Option<u64> {
    (bytes > 0).then_some(bytes / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_has_plausible_values_and_a_second_one_measures_the_cpu() {
        let mut sampler = Sampler::new();
        let first = sampler.sample();
        assert!(first.cpus.is_some_and(|cpus| cpus >= 1.0), "{first:?}");
        assert_eq!(
            first.cpu_percent, None,
            "one sample cannot measure CPU usage"
        );
        let total = first.memory_total_mb.expect("total memory");
        let available = first.memory_available_mb.expect("available memory");
        assert!(total > 0 && available <= total, "{first:?}");
        assert!(first.sampled_at > 0);
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        let second = sampler.sample();
        let percent = second
            .cpu_percent
            .expect("a second sample has a CPU percent");
        assert!((0.0..=100.0).contains(&percent), "{second:?}");
    }
}
