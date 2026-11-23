use std::time::{Duration, Instant};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct CpuUsage {
    last_sample: Option<(Instant, Duration)>,
    pub percent: Option<f64>,
}

impl CpuUsage {
    pub fn sample(&mut self) {
        let now = Instant::now();
        if self
            .last_sample
            .is_some_and(|(time, _)| now.duration_since(time) < SAMPLE_INTERVAL)
        {
            return;
        }

        let Some(cpu_time) = process_cpu_time() else {
            self.last_sample = None;
            self.percent = None;
            return;
        };
        if let Some((previous_time, previous_cpu)) = self.last_sample {
            self.percent = cpu_time.checked_sub(previous_cpu).map(|used| {
                100.0 * used.as_secs_f64() / now.duration_since(previous_time).as_secs_f64()
            });
        }
        self.last_sample = Some((now, cpu_time));
    }
}

#[cfg(windows)]
fn process_cpu_time() -> Option<Duration> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // The current process pseudo-handle does not need to be closed. All output pointers are valid.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    (ok != 0).then(|| Duration::from_nanos((ticks(kernel) + ticks(user)) * 100))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_cpu_time() -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // The initialized timespec is writable for the duration of the call.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) };
    (result == 0).then(|| Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn process_cpu_time() -> Option<Duration> {
    None
}
