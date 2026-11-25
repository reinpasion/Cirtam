use cirtam_core::{
    circuit::{Circuit, Kind},
    diagnostics::Diagnostic,
    solver::{self, Solution},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const REFRESH: Duration = Duration::from_millis(33);
const MAX_PACING_WAIT_SECONDS: f64 = 3600.0;
const HISTORY_LIMIT: usize = 1024;
// Keep adjacent step indices distinguishable when converted to f64.
const MAX_STEP: u64 = 1 << 48;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Timed,
    Live,
}

pub struct Settings {
    pub mode: Mode,
    pub timestep: f64,
    pub end: f64,
    pub save_from: f64,
    pub speed: f64,
    pub unlimited: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Timed,
            timestep: 0.1,
            end: 1.0,
            save_from: 0.0,
            speed: 1.0,
            unlimited: false,
        }
    }
}

struct Schedule {
    timestep: f64,
    end: Option<(u64, f64)>,
    save_from: f64,
    speed: Option<f64>,
}

impl Schedule {
    fn new(settings: &Settings) -> Result<Self, String> {
        let timestep = settings.timestep;
        if !timestep.is_finite() || timestep <= 0.0 {
            return Err("Timestep must be finite and greater than zero.".into());
        }
        let (end, save_from, speed) = match settings.mode {
            Mode::Timed => {
                let (end, save_from) = (settings.end, settings.save_from);
                if !(end.is_finite()
                    && save_from.is_finite()
                    && 0.0 <= save_from
                    && save_from <= end)
                {
                    return Err(
                        "Require finite times with 0 ≤ Save results from ≤ End time.".into(),
                    );
                }
                let steps = end / timestep;
                let rounded = steps.round();
                let near_integer =
                    (steps - rounded).abs() <= (steps * f64::EPSILON * 4.0).min(0.125);
                let last = if end == 0.0 {
                    0.0
                } else if near_integer {
                    rounded.max(1.0)
                } else {
                    steps.ceil().max(1.0)
                };
                if !last.is_finite() || last > MAX_STEP as f64 {
                    return Err("The end time and timestep require too many steps.".into());
                }
                (Some((last as u64, end)), save_from, None)
            }
            Mode::Live => {
                if !settings.unlimited && (!settings.speed.is_finite() || settings.speed <= 0.0) {
                    return Err("Live target speed must be finite and greater than zero.".into());
                }
                (None, 0.0, (!settings.unlimited).then_some(settings.speed))
            }
        };
        Ok(Self {
            timestep,
            end,
            save_from,
            speed,
        })
    }

    fn time(&self, step: u64) -> f64 {
        match self.end {
            Some((last, end)) if step == last => end,
            _ => step as f64 * self.timestep,
        }
    }

    fn retain(&self, time: f64) -> bool {
        time >= self.save_from || (self.save_from - time) <= time * f64::EPSILON * 4.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Running,
    Paused,
    Completed,
    Failed,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone)]
pub struct Sample {
    pub step: u64,
    pub time: f64,
    pub circuit: Arc<Circuit>,
    pub solution: Arc<Solution>,
}

#[derive(Clone)]
pub struct TraceSample {
    pub step: u64,
    pub time: f64,
    pub solution: Arc<Solution>,
}

#[derive(Clone)]
pub struct Snapshot {
    pub state: State,
    pub latest: Option<Sample>,
    pub history: VecDeque<TraceSample>,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub solve_duration: Option<Duration>,
    pub failed_time: Option<f64>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Running,
            latest: None,
            history: VecDeque::new(),
            diagnostics: Arc::default(),
            solve_duration: None,
            failed_time: None,
        }
    }
}

impl Snapshot {
    fn record(&mut self, sample: Sample, retain: bool) {
        if retain {
            if self.history.len() == HISTORY_LIMIT {
                self.history.pop_front();
            }
            self.history.push_back(TraceSample {
                step: sample.step,
                time: sample.time,
                solution: Arc::clone(&sample.solution),
            });
        }
        self.latest = Some(sample);
    }
}

enum Command {
    Pause,
    Resume,
    Stop,
    Source(usize, f64),
}

pub fn is_live_source(kind: Kind) -> bool {
    matches!(kind, Kind::Voltage | Kind::Current)
}

pub struct Run {
    commands: Sender<Command>,
    mailbox: Arc<Mutex<Option<Snapshot>>>,
    pub snapshot: Snapshot,
    pub mode: Mode,
    pub pause_pending: bool,
    component_kinds: Vec<Kind>,
}

impl Run {
    pub fn start(circuit: Circuit, settings: &Settings) -> Result<Self, String> {
        let schedule = Schedule::new(settings)?;
        let component_kinds = circuit.components.iter().map(|part| part.kind).collect();
        let (commands, rx) = mpsc::channel();
        let mailbox = Arc::new(Mutex::new(None));
        let output = Arc::clone(&mailbox);
        thread::Builder::new()
            .name("cirtam-dc-sampler".into())
            .spawn(move || worker(circuit, schedule, rx, output))
            .map_err(|error| format!("Could not start the DC sampler: {error}"))?;
        Ok(Self {
            commands,
            mailbox,
            snapshot: Snapshot::default(),
            mode: settings.mode,
            pause_pending: false,
            component_kinds,
        })
    }

    pub fn poll(&mut self) {
        let update = self
            .mailbox
            .try_lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(update) = update {
            if update.state != State::Running {
                self.pause_pending = false;
            }
            self.snapshot = update;
        }
    }

    pub fn pause(&mut self) {
        if self.snapshot.state == State::Running && !self.pause_pending {
            self.pause_pending = self.commands.send(Command::Pause).is_ok();
        }
    }

    pub fn resume(&mut self) {
        if self.snapshot.state == State::Paused && self.commands.send(Command::Resume).is_ok() {
            self.snapshot.state = State::Running;
        }
    }

    pub fn can_edit_sources(&self) -> bool {
        self.mode == Mode::Live && matches!(self.snapshot.state, State::Running | State::Paused)
    }

    pub fn queue_source(&self, index: usize, value: f64) -> Result<(), String> {
        if !self.can_edit_sources() {
            return Err("Source edits require an active Live run.".into());
        }
        let kind = self
            .component_kinds
            .get(index)
            .copied()
            .filter(|&kind| is_live_source(kind))
            .ok_or("Only independent DC source values can be edited during Live sampling.")?;
        kind.check_value(value)?;
        self.commands
            .send(Command::Source(index, value))
            .map_err(|_| "The Live run has already ended.".into())
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

struct Pace {
    origin_time: f64,
    wall_start: Instant,
}

impl Pace {
    fn wait(&self, time: f64, speed: f64, now: Instant) -> Duration {
        let seconds =
            (time - self.origin_time) / speed - now.duration_since(self.wall_start).as_secs_f64();
        Duration::from_secs_f64(seconds.clamp(0.0, MAX_PACING_WAIT_SECONDS))
    }
}

struct Publisher {
    mailbox: Arc<Mutex<Option<Snapshot>>>,
    last: Option<Instant>,
    pending: bool,
}

impl Publisher {
    fn publish(&mut self, snapshot: &Snapshot) {
        let update = snapshot.clone();
        *self.mailbox.lock().expect("DC sampler mailbox") = Some(update);
        self.last = Some(Instant::now());
        self.pending = false;
    }

    fn remaining(&self) -> Option<Duration> {
        self.pending.then(|| {
            self.last.map_or(Duration::ZERO, |last| {
                REFRESH.saturating_sub(last.elapsed())
            })
        })
    }
}

struct Worker {
    snapshot: Snapshot,
    next: u64,
    pace: Option<Pace>,
    edits: HashMap<usize, f64>,
}

impl Worker {
    fn pacing_wait(&mut self, time: f64, speed: f64, now: Instant) -> Duration {
        self.pace
            .get_or_insert(Pace {
                origin_time: time,
                wall_start: now,
            })
            .wait(time, speed, now)
    }

    fn command(&mut self, command: Command, publisher: &mut Publisher) -> bool {
        let before = self.snapshot.state;
        match command {
            Command::Stop => return false,
            Command::Pause => self.snapshot.state = State::Paused,
            Command::Resume if self.snapshot.state == State::Paused => {
                self.pace = None;
                self.snapshot.state = State::Running;
            }
            Command::Source(index, value) => {
                self.edits.insert(index, value);
            }
            _ => {}
        }
        if before != self.snapshot.state {
            publisher.publish(&self.snapshot);
        }
        true
    }
}

fn worker(
    mut circuit: Circuit,
    schedule: Schedule,
    commands: Receiver<Command>,
    mailbox: Arc<Mutex<Option<Snapshot>>>,
) {
    let mut run = Worker {
        snapshot: Snapshot::default(),
        next: 0,
        pace: None,
        edits: HashMap::new(),
    };
    let mut cached = None::<(Arc<Circuit>, Arc<Solution>)>;
    let mut publisher = Publisher {
        mailbox,
        last: None,
        pending: false,
    };

    loop {
        match commands.try_recv() {
            Ok(command) => {
                if !run.command(command, &mut publisher) {
                    return;
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        if run.snapshot.state == State::Paused {
            let Ok(command) = commands.recv() else { return };
            if !run.command(command, &mut publisher) {
                return;
            }
            continue;
        }

        if publisher.remaining().is_some_and(|wait| wait.is_zero()) {
            publisher.publish(&run.snapshot);
        }
        let time = schedule.time(run.next);
        if run.next > MAX_STEP || !time.is_finite() {
            run.snapshot.state = State::Failed;
            run.snapshot.diagnostics = Arc::new(vec![Diagnostic::serious(
                None,
                "The run reached its representable time range.",
            )]);
            publisher.publish(&run.snapshot);
            return;
        }
        if let Some(speed) = schedule.speed {
            let wait = run.pacing_wait(time, speed, Instant::now());
            if !wait.is_zero() {
                // A completed reading can become due for display before the next step.
                let wait = publisher
                    .remaining()
                    .map_or(wait, |refresh| wait.min(refresh));
                match commands.recv_timeout(wait) {
                    Ok(command) => {
                        if !run.command(command, &mut publisher) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                continue;
            }
        }
        for (index, value) in run.edits.drain() {
            let part = &mut circuit.components[index];
            if part.value != value {
                part.value = value;
                cached = None;
            }
        }
        if cached.is_none() {
            let started = Instant::now();
            let result = solver::solve(&circuit);
            run.snapshot.solve_duration = Some(started.elapsed());
            match result {
                Ok(mut solution) => {
                    run.snapshot.diagnostics = Arc::new(std::mem::take(&mut solution.diagnostics));
                    cached = Some((Arc::new(circuit.clone()), Arc::new(solution)));
                }
                Err(diagnostics) => {
                    run.snapshot.state = State::Failed;
                    run.snapshot.failed_time = Some(time);
                    run.snapshot.diagnostics = Arc::new(diagnostics);
                    publisher.publish(&run.snapshot);
                    return;
                }
            }
        }
        let (circuit, solution) = cached.as_ref().expect("a successful DC solve");
        run.snapshot.record(
            Sample {
                step: run.next,
                time,
                circuit: Arc::clone(circuit),
                solution: Arc::clone(solution),
            },
            schedule.retain(time),
        );
        if schedule.end.is_some_and(|(last, _)| last == run.next) {
            run.snapshot.state = State::Completed;
            publisher.publish(&run.snapshot);
            return;
        }
        publisher.pending = true;
        run.next += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirtam_core::persistence;

    fn divider() -> Circuit {
        persistence::decode(include_str!("../../../examples/divider.crtm")).unwrap()
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !done() {
            assert!(
                Instant::now() < deadline,
                "DC sampler did not reach the expected state"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn await_snapshot(run: &mut Run, ready: impl Fn(&Snapshot) -> bool) {
        wait_until(|| {
            run.poll();
            ready(&run.snapshot)
        });
    }

    #[test]
    fn timed_runs_start_at_zero_and_shorten_final_step() {
        let settings = Settings {
            timestep: 0.3,
            end: 1.0,
            save_from: 0.25,
            ..Settings::default()
        };
        let schedule = Schedule::new(&settings).unwrap();
        assert_eq!(schedule.time(0), 0.0);
        let mut run = Run::start(divider(), &settings).unwrap();
        await_snapshot(&mut run, |snapshot| snapshot.state == State::Completed);
        let latest = run.snapshot.latest.as_ref().unwrap();
        assert_eq!((latest.step, latest.time), (4, 1.0));
        assert_eq!(
            run.snapshot
                .history
                .iter()
                .map(|sample| sample.step)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        for (sample, expected) in run.snapshot.history.iter().zip([0.3, 0.6, 0.9, 1.0]) {
            assert!((sample.time - expected).abs() < 1e-12);
            assert!(Arc::ptr_eq(&sample.solution, &latest.solution));
        }
        assert!(run.queue_source(0, 12.0).is_err());
    }

    #[test]
    fn decimal_grid_alignment_and_invalid_settings() {
        let mut settings = Settings {
            end: 0.3,
            save_from: 0.3,
            ..Settings::default()
        };
        let schedule = Schedule::new(&settings).unwrap();
        assert_eq!(schedule.end, Some((3, 0.3)));
        assert!(!schedule.retain(0.2));
        assert!(schedule.retain(schedule.time(3)));
        settings.end = 0.0;
        settings.save_from = 0.0;
        assert_eq!(Schedule::new(&settings).unwrap().end, Some((0, 0.0)));
        settings.end = f64::MAX;
        assert!(Schedule::new(&settings).is_err());
        settings.end = 0.3;
        settings.save_from = 0.4;
        assert!(Schedule::new(&settings).is_err());
        settings.timestep = f64::NAN;
        assert!(Schedule::new(&settings).is_err());
    }

    #[test]
    fn pacing_uses_simulated_seconds_per_wall_second() {
        let now = Instant::now();
        let pace = Pace {
            origin_time: 2.0,
            wall_start: now,
        };
        assert!((pace.wait(2.04, 2.0, now).as_secs_f64() - 0.02).abs() < 1e-8);
        assert!(
            (pace
                .wait(2.04, 1.6, now + Duration::from_millis(10))
                .as_secs_f64()
                - 0.015)
                .abs()
                < 1e-8
        );
        assert!(pace
            .wait(2.04, 2.0, now + Duration::from_millis(30))
            .is_zero());
    }

    #[test]
    fn paced_deadlines_survive_late_ticks_and_resume_rebases() {
        let now = Instant::now();
        let mut worker = Worker {
            snapshot: Snapshot::default(),
            next: 0,
            pace: None,
            edits: HashMap::new(),
        };
        let mut publisher = Publisher {
            mailbox: Arc::new(Mutex::new(None)),
            last: None,
            pending: false,
        };
        assert!(worker.pacing_wait(0.0, 1.0, now).is_zero());
        let late = now + Duration::from_millis(108);
        assert!(worker.pacing_wait(0.1, 1.0, late).is_zero());
        assert_eq!(
            worker.pacing_wait(0.2, 1.0, late),
            Duration::from_millis(92)
        );

        assert!(worker.command(Command::Pause, &mut publisher));
        assert!(worker.command(Command::Resume, &mut publisher));
        let resumed = now + Duration::from_secs(10);
        assert!(worker.pacing_wait(0.2, 1.0, resumed).is_zero());
        assert_eq!(
            worker.pacing_wait(0.3, 1.0, resumed),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn live_pause_queues_edit_resume_advances_and_stop_retires_worker() {
        let settings = Settings {
            mode: Mode::Live,
            timestep: 60.0,
            speed: 2.0,
            ..Settings::default()
        };
        let design = divider();
        let mut run = Run::start(design.clone(), &settings).unwrap();
        await_snapshot(&mut run, |snapshot| snapshot.latest.is_some());
        assert_eq!(run.snapshot.latest.as_ref().unwrap().step, 0);
        assert!(run.queue_source(1, 12.0).is_err());
        assert!(run.queue_source(0, f64::NAN).is_err());
        run.pause();
        await_snapshot(&mut run, |snapshot| snapshot.state == State::Paused);
        assert_eq!(run.snapshot.latest.as_ref().unwrap().step, 0);
        run.queue_source(0, 20.0).unwrap();
        run.poll();
        let paused = run.snapshot.latest.as_ref().unwrap();
        assert_eq!((paused.step, paused.circuit.components[0].value), (0, 10.0));

        run.resume();
        await_snapshot(&mut run, |snapshot| {
            snapshot
                .latest
                .as_ref()
                .is_some_and(|sample| sample.step >= 1)
        });
        let resumed = run.snapshot.latest.as_ref().unwrap();
        assert_eq!((resumed.step, resumed.time), (1, 60.0));
        assert_eq!(resumed.circuit.components[0].value, 20.0);
        assert!((resumed.solution.readings[4].voltage - 10.0).abs() < 1e-10);
        assert_eq!(design.components[0].value, 10.0);

        let old_mailbox = Arc::clone(&run.mailbox);
        drop(run);
        let mut fresh = Run::start(design, &settings).unwrap();
        await_snapshot(&mut fresh, |snapshot| snapshot.latest.is_some());
        let restarted = fresh.snapshot.latest.as_ref().unwrap();
        assert_eq!(
            (
                restarted.step,
                restarted.time,
                restarted.circuit.components[0].value
            ),
            (0, 0.0, 10.0)
        );
        wait_until(|| Arc::strong_count(&old_mailbox) == 1);
    }

    #[test]
    fn live_unpaced_history_rolls_without_losing_run_state() {
        let settings = Settings {
            mode: Mode::Live,
            unlimited: true,
            speed: 0.0,
            ..Settings::default()
        };
        let mut run = Run::start(divider(), &settings).unwrap();
        await_snapshot(&mut run, |snapshot| {
            snapshot
                .history
                .front()
                .is_some_and(|sample| sample.step > 0)
        });
        run.pause();
        await_snapshot(&mut run, |snapshot| snapshot.state == State::Paused);
        let history = &run.snapshot.history;
        assert_eq!(history.len(), HISTORY_LIMIT);
        let first = history.front().unwrap();
        let last = history.back().unwrap();
        assert_eq!(last.step - first.step, HISTORY_LIMIT as u64 - 1);
        let latest = run.snapshot.latest.as_ref().unwrap();
        assert_eq!(latest.step, last.step);
        assert!(Arc::ptr_eq(&latest.solution, &first.solution));
    }

    #[test]
    fn failed_live_solve_keeps_the_last_good_reading() {
        let settings = Settings {
            mode: Mode::Live,
            timestep: 60.0,
            ..Settings::default()
        };
        let mut run = Run::start(divider(), &settings).unwrap();
        await_snapshot(&mut run, |snapshot| snapshot.latest.is_some());
        run.pause();
        await_snapshot(&mut run, |snapshot| snapshot.state == State::Paused);
        run.queue_source(0, f64::MAX).unwrap();
        run.resume();
        await_snapshot(&mut run, |snapshot| snapshot.state == State::Failed);
        assert_eq!(run.snapshot.failed_time, Some(60.0));
        assert_eq!(run.snapshot.latest.as_ref().unwrap().time, 0.0);
        assert!(!run.snapshot.diagnostics.is_empty());
    }
}
