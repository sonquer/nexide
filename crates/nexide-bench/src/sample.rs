//! Background sampler reading CPU% and RSS of a target PID via
//! `sysinfo` on a fixed interval.
//!
//! The sampler runs as a Tokio task; it is started with
//! [`ProcessSampler::spawn`] and consumed by [`ProcessSampler::stop`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// Aggregated CPU/memory statistics over the sampled window.
#[derive(Debug, Clone, Default)]
pub struct SampleStats {
    /// Average CPU usage in percent (sum across cores; can exceed 100%).
    pub cpu_avg: f64,
    /// Maximum observed CPU usage in percent.
    pub cpu_max: f64,
    /// Average resident memory in megabytes.
    pub mem_avg_mb: f64,
    /// Maximum observed resident memory in megabytes.
    pub mem_max_mb: f64,
    /// Maximum observed thread count.
    pub threads_max: u64,
    /// Number of samples taken.
    pub samples: u64,
}

/// Async sampler that polls `/proc/<pid>` (or platform equivalent)
/// at a fixed interval and collects CPU/RSS/thread stats.
pub struct ProcessSampler {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<SampleStats>,
}

#[derive(Default)]
struct Accumulator {
    cpu_sum: f64,
    cpu_max: f64,
    mem_sum: f64,
    mem_max: f64,
    threads_max: u64,
    samples: u64,
}

impl Accumulator {
    fn record(&mut self, cpu: f64, mem_mb: f64, threads: u64) {
        self.cpu_sum += cpu;
        if cpu > self.cpu_max {
            self.cpu_max = cpu;
        }
        self.mem_sum += mem_mb;
        if mem_mb > self.mem_max {
            self.mem_max = mem_mb;
        }
        if threads > self.threads_max {
            self.threads_max = threads;
        }
        self.samples += 1;
    }

    fn finish(self) -> SampleStats {
        let div = self.samples.max(1) as f64;
        SampleStats {
            cpu_avg: self.cpu_sum / div,
            cpu_max: self.cpu_max,
            mem_avg_mb: self.mem_sum / div,
            mem_max_mb: self.mem_max,
            threads_max: self.threads_max,
            samples: self.samples,
        }
    }
}

#[cfg(target_os = "linux")]
fn current_thread_count(pid: u32) -> u64 {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|d| d.count() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn current_thread_count(pid: u32) -> u64 {
    use std::process::Command;
    let out = Command::new("ps")
        .args(["-M", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .count() as u64,
        _ => 0,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn current_thread_count(_pid: u32) -> u64 {
    0
}

impl ProcessSampler {
    /// Spawn a sampler for `pid` polling at `interval`.
    ///
    /// # Errors
    /// Returns an error when the process cannot be observed.
    pub fn spawn(pid: u32, interval: Duration) -> Result<Self> {
        let mut system = System::new();
        let sys_pid = Pid::from_u32(pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::everything(),
        );
        if system.process(sys_pid).is_none() {
            bail!("pid {pid} not found");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_inner = Arc::clone(&stop);
        let acc: Arc<Mutex<Accumulator>> = Arc::new(Mutex::new(Accumulator::default()));
        let acc_inner = Arc::clone(&acc);
        let handle = tokio::spawn(async move {
            let mut system = system;
            while !stop_inner.load(Ordering::Relaxed) {
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[sys_pid]),
                    true,
                    ProcessRefreshKind::everything(),
                );
                if let Some(proc) = system.process(sys_pid) {
                    let cpu = f64::from(proc.cpu_usage());
                    let mem_mb = proc.memory() as f64 / 1024.0 / 1024.0;
                    let threads = current_thread_count(pid);
                    acc_inner.lock().await.record(cpu, mem_mb, threads);
                } else {
                    break;
                }
                sleep(interval).await;
            }
            let inner = std::mem::take(&mut *acc_inner.lock().await);
            inner.finish()
        });
        Ok(Self { stop, handle })
    }

    /// Signal the sampler to stop and await the final aggregate.
    ///
    /// # Errors
    /// Returns an error when the sampler task panics.
    pub async fn stop(self) -> Result<SampleStats> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.await.context("sampler join")
    }
}
