use crate::engine::Engine;
use crate::error::ApiError;
use crate::recording::SimConfig;
use crate::sim::Simulator;
use crate::types::{Command, Event, Query, Response, Status};
use board_zybo::{BoardDef, BoardState};
use sim_core::Ps;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Wall-clock length of one continuous-run slice. Pacing only: simulated time
/// advances by a fixed amount per slice, so results do not depend on it.
const SLICE: Duration = Duration::from_millis(10);
const SLICES_PER_SECOND: u64 = 100;
/// Longest stretch of simulated time run between two checks of the request
/// queue while playing.
const MAX_CHUNK_PS: u64 = 200 * 1_000_000;
/// Wall-clock budget of one playing tick before requests are served again.
const TICK_BUDGET: Duration = Duration::from_millis(20);

enum Msg {
    Command(Command, Sender<Result<Response, ApiError>>),
    Query(Query, Sender<Result<Response, ApiError>>),
    Subscribe(Sender<Event>),
}

/// A cloneable handle to a simulation running on its own thread.
///
/// This is the single control plane: the GUI, the CLI and the MCP server all
/// hold a `SimClient`. Calls block only for the duration of one request, and
/// the simulation never waits for the caller (requirement §11).
#[derive(Clone)]
pub struct SimClient {
    tx: Sender<Msg>,
}

impl SimClient {
    /// Starts a simulation thread for `config`.
    pub fn spawn(config: SimConfig) -> Result<SimClient, ApiError> {
        Ok(SimClient::spawn_with(Simulator::new(config)?))
    }

    /// Starts a simulation thread around an existing demo simulator.
    pub fn spawn_with(sim: Simulator) -> SimClient {
        SimClient::spawn_engine(Box::new(sim))
    }

    /// Starts a simulation thread around any engine.
    pub fn spawn_engine(sim: Box<dyn Engine>) -> SimClient {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("zynqsim-sim".to_string())
            .spawn(move || serve(sim, rx))
            .unwrap_or_else(|e| panic!("cannot start the simulation thread: {e}"));
        SimClient { tx }
    }

    /// Runs a state-changing command.
    pub fn command(&self, cmd: Command) -> Result<Response, ApiError> {
        let (rtx, rrx) = mpsc::channel();
        self.tx
            .send(Msg::Command(cmd, rtx))
            .map_err(|_| ApiError::disconnected())?;
        rrx.recv().map_err(|_| ApiError::disconnected())?
    }

    /// Runs a read-only query.
    pub fn query(&self, q: Query) -> Result<Response, ApiError> {
        let (rtx, rrx) = mpsc::channel();
        self.tx
            .send(Msg::Query(q, rtx))
            .map_err(|_| ApiError::disconnected())?;
        rrx.recv().map_err(|_| ApiError::disconnected())?
    }

    /// Subscribes to the event stream. Events are delivered from the moment of
    /// subscription; the receiver disconnects when the simulation ends.
    pub fn subscribe(&self) -> Result<Receiver<Event>, ApiError> {
        let (etx, erx) = mpsc::channel();
        self.tx
            .send(Msg::Subscribe(etx))
            .map_err(|_| ApiError::disconnected())?;
        Ok(erx)
    }

    /// Typed helper for [`Query::Status`].
    pub fn status(&self) -> Result<Status, ApiError> {
        match self.query(Query::Status)? {
            Response::Status(s) => Ok(*s),
            r => Err(unexpected(&r)),
        }
    }

    /// Typed helper for [`Query::BoardDef`].
    pub fn board_def(&self) -> Result<BoardDef, ApiError> {
        match self.query(Query::BoardDef)? {
            Response::BoardDef(d) => Ok(*d),
            r => Err(unexpected(&r)),
        }
    }

    /// Typed helper for [`Query::BoardState`].
    pub fn board_state(&self) -> Result<BoardState, ApiError> {
        match self.query(Query::BoardState)? {
            Response::BoardState(s) => Ok(s),
            r => Err(unexpected(&r)),
        }
    }
}

fn unexpected(r: &Response) -> ApiError {
    ApiError::internal(format!("unexpected response variant: {r:?}"))
}

/// Publishes trace records appended since `*published` to all subscribers.
fn publish(sim: &dyn Engine, published: &mut usize, subs: &mut Vec<Sender<Event>>) {
    let log = sim.trace();
    let records = log.records();
    let base = log.first_index();
    // Records dropped before they were published are gone: skip to the oldest.
    *published = (*published).max(base);
    while *published < base + records.len() {
        let ev = Event::Trace {
            index: *published,
            record: records[*published - base].clone(),
        };
        subs.retain(|s| s.send(ev.clone()).is_ok());
        *published += 1;
    }
}

fn broadcast(subs: &mut Vec<Sender<Event>>, ev: &Event) {
    subs.retain(|s| s.send(ev.clone()).is_ok());
}

fn serve(mut sim: Box<dyn Engine>, rx: Receiver<Msg>) {
    let mut subs: Vec<Sender<Event>> = Vec::new();
    // Simulated picoseconds per wall-clock second while playing.
    let mut playing: Option<u64> = None;
    let mut next_slice = Instant::now();
    let mut published = sim.trace().len();

    loop {
        let msg = if playing.is_some() {
            let wait = next_slice.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };

        match msg {
            Some(Msg::Subscribe(tx)) => subs.push(tx),
            Some(Msg::Query(q, reply)) => {
                let _ = reply.send(sim.query(&q, playing.is_some()));
            }
            Some(Msg::Command(cmd, reply)) => {
                let result = match cmd {
                    Command::Play { ps_per_second } => {
                        if ps_per_second == 0 {
                            Err(ApiError::invalid_argument("ps_per_second must be non-zero"))
                        } else {
                            if playing.is_none() {
                                next_slice = Instant::now() + SLICE;
                                broadcast(&mut subs, &Event::Playing { playing: true });
                            }
                            playing = Some(ps_per_second);
                            Ok(Response::Ok)
                        }
                    }
                    Command::Pause => {
                        if playing.take().is_some() {
                            broadcast(&mut subs, &Event::Playing { playing: false });
                        }
                        Ok(Response::Ok)
                    }
                    other => {
                        let restored = matches!(other, Command::SnapshotRestore { .. });
                        let reset = matches!(other, Command::Input(crate::Input::Reset { .. }));
                        let r = sim.command(other);
                        if (restored || reset) && r.is_ok() {
                            published = sim.trace().len();
                        }
                        if restored && r.is_ok() {
                            broadcast(&mut subs, &Event::Restored);
                        }
                        r
                    }
                };
                let _ = reply.send(result);
            }
            None => {}
        }

        if let Some(pps) = playing {
            let now = Instant::now();
            if now >= next_slice {
                // One slice of simulated time per tick, no catch-up: a slow
                // host slows simulated time instead of changing what is
                // simulated. The slice is run in small chunks and abandoned
                // early when the wall-clock budget is spent, so a slow engine
                // (RTL in XSim) cannot keep queries waiting for seconds.
                let want = pps / SLICES_PER_SECOND;
                let started = Instant::now();
                let mut done = 0u64;
                while done < want {
                    let chunk = (want - done).min(MAX_CHUNK_PS);
                    if let Err(e) = sim.run_for(Ps(chunk)) {
                        // A faulting engine stops continuous running and says why.
                        playing = None;
                        broadcast(&mut subs, &Event::Playing { playing: false });
                        broadcast(&mut subs, &Event::Fault { message: e.message });
                        break;
                    }
                    done += chunk;
                    if started.elapsed() > TICK_BUDGET {
                        break;
                    }
                }
                next_slice = now + SLICE;
            }
        }
        publish(sim.as_ref(), &mut published, &mut subs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Input;
    use std::time::Duration;

    #[test]
    fn commands_queries_and_errors_round_trip() {
        let c = SimClient::spawn(SimConfig::default()).unwrap();
        c.command(Command::Input(Input::SetSwitch { index: 0, on: true }))
            .unwrap();
        let s = c.board_state().unwrap();
        assert!(s.switches[0] && s.leds[0] == 255);
        let st = c.status().unwrap();
        assert_eq!(st.ps_backend, "demo");
        assert!(!st.playing);
        let e = c
            .command(Command::Input(Input::SetSwitch {
                index: 40,
                on: true,
            }))
            .unwrap_err();
        assert_eq!(e.code, crate::ErrorCode::InvalidArgument);
        assert_eq!(c.board_def().unwrap().name, "zybo-z7-20");
    }

    #[test]
    fn events_stream_trace_records_and_state_changes() {
        let c = SimClient::spawn(SimConfig::default()).unwrap();
        let rx = c.subscribe().unwrap();
        c.command(Command::Input(Input::SetSwitch { index: 1, on: true }))
            .unwrap();
        let mut got = Vec::new();
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
            got.push(ev);
        }
        // Input note + switch signal + led1 signal.
        assert_eq!(got.len(), 4, "{got:?}"); // note, sw1, led1, led5 (RGB)
        assert!(matches!(got[0], Event::Trace { .. }));
    }

    #[test]
    fn play_advances_time_and_pause_stops_it() {
        let c = SimClient::spawn(SimConfig::default()).unwrap();
        let rx = c.subscribe().unwrap();
        c.command(Command::Play {
            ps_per_second: Ps::PER_S, // real time
        })
        .unwrap();
        assert!(c.status().unwrap().playing);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Playing { playing: true }
        );
        std::thread::sleep(Duration::from_millis(150));
        c.command(Command::Pause).unwrap();
        let t0 = c.status().unwrap().now_ps;
        assert!(t0 > 0, "time should have advanced");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.status().unwrap().now_ps, t0, "paused time must not move");
        assert!(c.command(Command::Play { ps_per_second: 0 }).is_err());
    }

    #[test]
    fn snapshot_restore_emits_restored_and_rewinds() {
        let c = SimClient::spawn(SimConfig::default()).unwrap();
        c.command(Command::SnapshotSave { name: "s".into() })
            .unwrap();
        c.command(Command::Run {
            duration_ps: Ps::PER_S,
        })
        .unwrap();
        let rx = c.subscribe().unwrap();
        c.command(Command::SnapshotRestore { name: "s".into() })
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Restored
        );
        assert_eq!(c.status().unwrap().now_ps, 0);
    }

    #[test]
    fn calls_after_all_clients_drop_do_not_hang() {
        let c = SimClient::spawn(SimConfig::default()).unwrap();
        let c2 = c.clone();
        drop(c);
        assert!(c2.status().is_ok());
    }
}
