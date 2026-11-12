//! The running plugin host: loads enabled contributions from the shared store, runs session
//! plugins on worker threads and serves their document ops on the session's thread.
//!
//! Threading (B4): plugin code never runs on the session's thread. Each (plugin, module) session —
//! or native helper — lives on its own worker and receives events in order through a queue. Its
//! host calls (`AppApi::begin/call/finish`) become [`Request`]s to the session's thread, which serves
//! them in [`Runtime::pump`] (each frame in the app; inline, blocking, in the CLI). One event is one
//! transaction: its whole begin → ops → finish sequence is served in one pump, so no other edit can
//! interleave, and it becomes one undo step `"<label> · <plugin name>"` or is rolled back.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use craft_ext_core::id::{AppId, ExtensionId};
use craft_ext_core::manifest::{Contribution, ContributionKind, Manifest};
use craft_ext_core::permission::Permission;
use craft_ext_core::project::{PluginRef, Source, Status};
use craft_ext_core::version::Version;
use craft_ext_host::api::{AppApi, AppContract, Code, Grants, Invocation, OpError};
use craft_ext_host::wasm::{LogFn, MAX_MODULE_BYTES, Plugin, Session as WasmSession};
use craft_ext_store::Manager;
use craft_ext_store::state::Origin;
use craft_ext_store::view::GrantView;
use effectcraft_engine::ext::{PluginCommand, PluginRuntime, PluginSurface};
use effectcraft_engine::{EngineError, Session};
use effectcraft_project::{LayerId, Project};
use serde_json::{Value, json};

use crate::contract::{APP, CAPTURED_SELECTION, contract, provided};
use crate::effects::{FilterEffect, param_spec};
use crate::grants::StoreGrants;
use crate::ops::{self, Outcome, Scope, UiChange};

/// How long a worker waits for the session's thread to serve one request.
const OP_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest one event's transaction may hold the session's thread.
const TX_TIMEOUT: Duration = Duration::from_secs(5);
/// Deadline of one event (fuel slices check it).
const EVENT_TIMEOUT: Duration = Duration::from_secs(8);
/// Events queued per worker; app events beyond it are dropped.
const QUEUE: usize = 64;
const MAX_WINDOWS: usize = 4;
const MAX_DATA_BYTES: u64 = 4 * 1024 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A host call from a worker, served on the session's thread.
struct Request {
    /// Identity of the invocation (its cancel flag's address).
    key: usize,
    plugin: ExtensionId,
    /// Undo label of the event ("<label> · <plugin name>").
    label: String,
    contribution: String,
    /// What the invoking contribution declared (windows it opens keep it for their events).
    declared: Arc<[Permission]>,
    kind: Kind,
    reply: Sender<Result<Value, OpError>>,
    sent: Instant,
}

enum Kind {
    Begin,
    Call(String, Value),
    Finish(bool),
}

fn inv_key(inv: &Invocation) -> usize {
    Arc::as_ptr(&inv.cancel) as usize
}

/// The `AppApi` a worker's session sees: forwards to the session's thread and waits.
struct Bridge {
    tx: Sender<Request>,
}

impl Bridge {
    fn ask(&self, inv: &Invocation, kind: Kind) -> Result<Value, OpError> {
        let (reply, rx) = mpsc::channel();
        let req = Request {
            key: inv_key(inv),
            plugin: inv.plugin.clone(),
            label: inv.label.clone(),
            contribution: inv.contribution.clone(),
            declared: inv.declared.clone(),
            kind,
            reply,
            sent: Instant::now(),
        };
        self.tx.send(req).map_err(|_| OpError::new(Code::Cancelled, "EffectCraft is closing"))?;
        match rx.recv_timeout(OP_TIMEOUT) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(OpError::new(Code::Stale, "EffectCraft is busy; try again")),
            Err(RecvTimeoutError::Disconnected) => Err(OpError::new(Code::Cancelled, "the request was dropped")),
        }
    }
}

impl AppApi for Bridge {
    fn contract(&self) -> &AppContract {
        contract()
    }
    fn begin(&self, inv: &Invocation) -> Result<(), OpError> {
        let captured = self.ask(inv, Kind::Begin)?;
        let ids: Vec<u64> = captured["selection"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        CAPTURED_SELECTION.with(|s| *s.borrow_mut() = ids);
        Ok(())
    }
    fn call(&self, inv: &Invocation, op: &str, args: Value) -> Result<Value, OpError> {
        self.ask(inv, Kind::Call(op.to_string(), args))
    }
    fn finish(&self, inv: &Invocation, commit: bool) {
        if let Err(e) = self.ask(inv, Kind::Finish(commit)) {
            log::warn!("plugin {}: finishing the event failed: {e}", inv.plugin);
        }
        CAPTURED_SELECTION.with(|s| s.borrow_mut().clear());
    }
}

enum Job {
    Event { inv: Invocation, event: Value, done: Option<Sender<Result<(), String>>> },
    Stop,
}

/// What a worker runs.
enum Program {
    Wasm(Plugin),
    Native { plugin: ExtensionId, version: Version, exe: PathBuf, work: PathBuf },
}

struct Worker {
    tx: SyncSender<Job>,
}

/// One loaded (active, enabled) release.
struct Loaded {
    id: ExtensionId,
    version: Version,
    name: String,
    sha256: Option<String>,
    source: Source,
    manifest: Manifest,
    enabled: BTreeSet<String>,
}

/// A session contribution (command, panel, event hook) and the worker that runs it.
#[derive(Clone)]
struct SessionContribution {
    plugin: ExtensionId,
    version: Version,
    plugin_name: String,
    contribution: String,
    label: String,
    kind: ContributionKind,
    category: Option<String>,
    worker: String,
    /// Required ∪ optional permissions of this contribution.
    declared: Arc<[Permission]>,
}

/// A preset from a `preset` contribution (applied on the session's thread; no code runs).
#[derive(Clone)]
struct Preset {
    command: String,
    plugin: ExtensionId,
    plugin_name: String,
    contribution: String,
    id: String,
    label: String,
    effects: Vec<(String, serde_json::Map<String, Value>)>,
}

#[derive(Clone)]
struct Lut {
    plugin: ExtensionId,
    contribution: String,
    label: String,
    path: PathBuf,
}

struct Surface {
    title: String,
    tree: Option<Value>,
    rev: u64,
    window: bool,
    open: bool,
    /// The worker that owns it (UI events go there).
    worker: String,
    /// The contribution whose rights its events carry.
    contribution: String,
    declared: Arc<[Permission]>,
    plugin: ExtensionId,
    version: Version,
    plugin_name: String,
}

#[derive(Default)]
struct State {
    loaded: Vec<Loaded>,
    sessions: Vec<SessionContribution>,
    workers: HashMap<String, Worker>,
    /// Workers whose plugin may receive app events (has an event hook).
    hooks: BTreeSet<String>,
    surfaces: BTreeMap<String, Surface>,
    effects: Vec<String>,
    presets: Vec<Preset>,
    luts: Vec<Lut>,
    store_changes: u64,
}

/// What the session last looked like, for app events.
#[derive(Default)]
struct Seen {
    selection: Vec<LayerId>,
    time: Option<f64>,
    time_sent: Option<Instant>,
    rendering: bool,
    /// A project was opened (`document.opened` goes out on the next pump).
    opened: Option<String>,
}

pub struct Runtime {
    app: AppId,
    root: PathBuf,
    grants: Arc<StoreGrants>,
    safe: bool,
    /// Set by the first [`PluginRuntime::pump`]: a frontend serves requests every frame, so
    /// commands return at once instead of blocking.
    interactive: AtomicBool,
    state: Mutex<State>,
    requests: Mutex<Receiver<Request>>,
    tx: Sender<Request>,
    deferred: Mutex<VecDeque<Request>>,
    /// Results of events started by interactive commands (errors become toasts).
    results: Mutex<(Sender<Result<(), String>>, Receiver<Result<(), String>>)>,
    seen: Mutex<Seen>,
    prompt: Mutex<Option<Vec<(PluginRef, Status)>>>,
    log: LogFn,
    next_rev: AtomicU64,
}

fn make_log() -> LogFn {
    Arc::new(|plugin: &str, level: i32, text: &str| {
        let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').take(1000).collect();
        match level {
            3 => log::error!("plugin {plugin}: {text}"),
            2 => log::warn!("plugin {plugin}: {text}"),
            1 => log::info!("plugin {plugin}: {text}"),
            _ => log::debug!("plugin {plugin}: {text}"),
        }
    })
}

/// The platform name native packages use (`linux-x86_64` …).
pub fn platform() -> String {
    craft_ext_host::native::platform()
}

impl Runtime {
    pub fn new(root: PathBuf, safe: bool) -> Result<Self, String> {
        let app = AppId::new(APP).map_err(|e| e.to_string())?;
        let view = GrantView::open(&root, app.clone()).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            app,
            root,
            grants: Arc::new(StoreGrants::new(view)),
            safe,
            interactive: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            requests: Mutex::new(rx),
            tx,
            deferred: Mutex::new(VecDeque::new()),
            results: Mutex::new(mpsc::channel()),
            seen: Mutex::new(Seen::default()),
            prompt: Mutex::new(None),
            log: make_log(),
            next_rev: AtomicU64::new(1),
        })
    }

    /// A manager over the same store, for the UI and CLI.
    pub fn manager(&self) -> Manager {
        Manager::new(craft_ext_store::Store::at(&self.root), Some(self.app.clone()), provided(), platform())
    }

    pub fn safe_mode(&self) -> bool {
        self.safe
    }

    /// The project-open statuses waiting for the "This project uses N plugins" prompt.
    pub fn take_project_prompt(&self) -> Option<Vec<(PluginRef, Status)>> {
        lock(&self.prompt).take()
    }

    /// Loads every enabled contribution (effects register at once; sessions start their worker
    /// and receive `init`). Safe mode loads nothing.
    pub fn load(&self, s: &mut Session) {
        let mut st = lock(&self.state);
        st.store_changes = self.grants.changes();
        if self.safe {
            return;
        }
        let enabled = match self.grants.view().enabled_contributions(&self.app) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("plugins: {e}");
                return;
            }
        };
        let store_state = craft_ext_store::Store::at(&self.root).state().ok();
        let provided = provided();
        for (id, version, dir, manifest, on) in enabled {
            let rec = store_state.as_ref().and_then(|s| s.packages.get(&id));
            let sha256 = rec.and_then(|p| p.releases.get(&version)).map(|r| r.sha256.clone());
            let source = match rec.map(|p| &p.origin) {
                Some(Origin::Catalogue { .. }) => Source::Catalogue,
                _ => Source::Local,
            };
            let loaded = Loaded { id: id.clone(), version, name: manifest.name.clone(), sha256, source, manifest: manifest.clone(), enabled: on.clone() };
            for c in manifest.contributions_for(&self.app).filter(|c| on.contains(&c.id)) {
                if let Some(why) = c.contract.check(&provided) {
                    log::info!("plugin {id}: {} is unavailable here: {why}", c.label);
                    continue;
                }
                if let Err(e) = self.load_contribution(&mut st, &loaded, &dir, c) {
                    log::warn!("plugin {id}: {}: {e}", c.label);
                    effectcraft_engine::ext::notify(s, format!("{}: {} could not load: {e}", manifest.name, c.label), true);
                }
            }
            st.loaded.push(loaded);
        }
        // Initialize each contribution with only its own declared permissions.
        // Modules may be shared, but that must never merge contribution authority.
        let starts: Vec<_> = st
            .sessions
            .iter()
            .map(|c| (c.worker.clone(), c.plugin.clone(), c.version, c.plugin_name.clone(), c.contribution.clone(), c.declared.clone()))
            .collect();
        for (worker, plugin, version, name, contribution, declared) in starts {
            {
                let inv = self.invocation(&plugin, version, &contribution, &format!("Start · {name}"), declared);
                if let Some(w) = st.workers.get(&worker) {
                    let _ = w.tx.try_send(Job::Event { inv, event: json!({"event": "init"}), done: None });
                }
            }
        }
        drop(st);
        s.layer_cache.clear();
    }

    fn load_contribution(&self, st: &mut State, p: &Loaded, dir: &Path, c: &Contribution) -> Result<(), String> {
        match c.kind {
            ContributionKind::Effect => {
                let module = self.module(p, dir, c)?;
                let id = format!("{}/{}", p.id, c.id);
                let render = FilterEffect {
                    key: format!("{}@{}/{}", p.id, p.version, c.id).into(),
                    plugin: p.id.clone(),
                    version: p.version,
                    contribution: c.id.clone(),
                    required: c.permissions.clone(),
                    module,
                    params: c.params.clone(),
                    grants: self.grants.clone(),
                    log: self.log.clone(),
                };
                let category = c.category.clone().unwrap_or_else(|| "Plugins".into());
                effectcraft_engine::effects::external::register(&id, &c.label, &category, c.params.iter().map(param_spec).collect(), Arc::new(render))?;
                st.effects.push(id);
                Ok(())
            }
            ContributionKind::Command | ContributionKind::Panel | ContributionKind::EventHook => {
                let worker = match &p.manifest.native {
                    Some(n) => {
                        let rel = n.executables.get(&platform()).ok_or("no native executable for this platform")?;
                        let key = format!("{}@{}#native", p.id, p.version);
                        if !st.workers.contains_key(&key) {
                            let work = craft_ext_store::Store::at(&self.root).private_dir(&self.app, &p.id);
                            std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                            let w = self.spawn(&key, Program::Native { plugin: p.id.clone(), version: p.version, exe: dir.join(rel), work })?;
                            st.workers.insert(key.clone(), w);
                        }
                        key
                    }
                    None => {
                        let module = c.module.as_deref().ok_or("no module")?;
                        let key = format!("{}@{}#{module}", p.id, p.version);
                        if !st.workers.contains_key(&key) {
                            let code = Program::Wasm(self.module(p, dir, c)?);
                            let w = self.spawn(&key, code)?;
                            st.workers.insert(key.clone(), w);
                        }
                        key
                    }
                };
                if c.kind == ContributionKind::EventHook {
                    st.hooks.insert(worker.clone());
                }
                let declared: Arc<[Permission]> = c.permissions.iter().chain(c.optional_permissions.iter()).cloned().collect::<Vec<_>>().into();
                if c.kind == ContributionKind::Panel {
                    st.surfaces.insert(
                        format!("{}/{}", p.id, c.id),
                        Surface {
                            title: c.label.clone(),
                            tree: None,
                            rev: 0,
                            window: false,
                            open: false,
                            worker: worker.clone(),
                            contribution: c.id.clone(),
                            declared: declared.clone(),
                            plugin: p.id.clone(),
                            version: p.version,
                            plugin_name: p.name.clone(),
                        },
                    );
                }
                st.sessions.push(SessionContribution {
                    plugin: p.id.clone(),
                    version: p.version,
                    plugin_name: p.name.clone(),
                    contribution: c.id.clone(),
                    label: c.label.clone(),
                    kind: c.kind,
                    category: c.category.clone(),
                    worker,
                    declared,
                });
                Ok(())
            }
            ContributionKind::Preset => {
                for f in &c.files {
                    let bytes = read_capped(&dir.join(f))?;
                    let doc: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{f}: {e}"))?;
                    for (i, pr) in doc["presets"].as_array().into_iter().flatten().take(256).enumerate() {
                        let pid = pr["id"].as_str().filter(|s| local(s)).map_or_else(|| format!("preset{i}"), str::to_string);
                        let label = pr["label"].as_str().map(|s| s.chars().take(64).collect()).unwrap_or_else(|| pid.clone());
                        let effects = pr["effects"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .take(32)
                            .filter_map(|e| Some((e["effect"].as_str()?.to_string(), e["params"].as_object().cloned().unwrap_or_default())))
                            .collect();
                        st.presets.push(Preset {
                            command: format!("plugin.{}.{}.{pid}", p.id, c.id),
                            plugin: p.id.clone(),
                            plugin_name: p.name.clone(),
                            contribution: c.id.clone(),
                            id: pid,
                            label,
                            effects,
                        });
                    }
                }
                Ok(())
            }
            ContributionKind::Lut => {
                for f in &c.files {
                    let path = dir.join(f);
                    if !path.is_file() {
                        return Err(format!("{f} is missing"));
                    }
                    st.luts.push(Lut { plugin: p.id.clone(), contribution: c.id.clone(), label: c.label.clone(), path });
                }
                Ok(())
            }
            // Tools, fonts and themes are not provided in this build (themes are deferred, B12).
            _ => Err(format!("{} contributions are not supported here yet", c.kind.label())),
        }
    }

    fn module(&self, p: &Loaded, dir: &Path, c: &Contribution) -> Result<Plugin, String> {
        let rel = c.module.as_deref().ok_or("no module")?;
        let path = dir.join(rel);
        let len = std::fs::metadata(&path).map_err(|e| format!("{rel}: {e}"))?.len();
        if len > MAX_MODULE_BYTES as u64 {
            return Err(format!("{rel} is too large"));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
        Plugin::load(p.id.as_str(), &bytes).map_err(|e| e.to_string())
    }

    fn spawn(&self, key: &str, code: Program) -> Result<Worker, String> {
        let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE);
        let api: Arc<dyn AppApi> = Arc::new(Bridge { tx: self.tx.clone() });
        let grants: Arc<dyn Grants> = self.grants.clone();
        let log = self.log.clone();
        let name = key.to_string();
        std::thread::Builder::new().name(format!("plugin {key}")).spawn(move || worker_main(&name, code, rx, api, grants, log)).map_err(|e| e.to_string())?;
        Ok(Worker { tx })
    }

    fn invocation(&self, plugin: &ExtensionId, version: Version, contribution: &str, label: &str, declared: Arc<[Permission]>) -> Invocation {
        Invocation {
            app: self.app.clone(),
            plugin: plugin.clone(),
            version,
            contribution: contribution.to_string(),
            grant_generation: self.grants.generation(plugin),
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Some(Instant::now() + EVENT_TIMEOUT),
            label: label.to_string(),
            declared,
        }
    }

    /// Stops every worker and unregisters every effect (instances pass through with a badge).
    pub fn unload(&self, why: &str) {
        let mut st = lock(&self.state);
        for (_, w) in st.workers.drain() {
            let _ = w.tx.try_send(Job::Stop);
        }
        for id in st.effects.drain(..) {
            effectcraft_engine::effects::external::unregister(&id, why);
        }
        let changes = st.store_changes;
        *st = State { store_changes: changes, ..State::default() };
    }

    /// Sends `event` to a worker. Interactive: returns at once (errors arrive as toasts); otherwise
    /// serves the event's requests here until it finishes.
    fn deliver(&self, s: &mut Session, worker: &str, inv: Invocation, event: Value, wait: bool) -> Result<(), String> {
        let (done_tx, done_rx) = mpsc::channel();
        let done = if wait { Some(done_tx) } else { Some(lock(&self.results).0.clone()) };
        {
            let st = lock(&self.state);
            let w = st.workers.get(worker).ok_or("the plugin is not running")?;
            match w.tx.try_send(Job::Event { inv, event, done }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => return Err("the plugin is busy".into()),
                Err(TrySendError::Disconnected(_)) => return Err("the plugin stopped".into()),
            }
        }
        if !wait {
            return Ok(());
        }
        let deadline = Instant::now() + EVENT_TIMEOUT + TX_TIMEOUT;
        loop {
            match done_rx.try_recv() {
                Ok(r) => return r,
                Err(mpsc::TryRecvError::Disconnected) => return Err("the plugin stopped".into()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() > deadline {
                return Err("the plugin took too long".into());
            }
            self.serve(s, Duration::from_millis(20));
        }
    }

    /// Serves queued requests; waits up to `wait` for the first one.
    fn serve(&self, s: &mut Session, wait: Duration) {
        let rx = lock(&self.requests);
        let mut deferred = lock(&self.deferred);
        if deferred.is_empty() {
            match rx.recv_timeout(wait) {
                Ok(r) => deferred.push_back(r),
                Err(_) => return,
            }
        }
        while let Ok(r) = rx.try_recv() {
            deferred.push_back(r);
        }
        // Each Begin runs its whole transaction; requests of other invocations wait.
        let pending: Vec<Request> = deferred.drain(..).collect();
        for r in pending {
            match r.kind {
                Kind::Begin => self.transaction(s, r, &rx, &mut deferred),
                Kind::Call(..) => {
                    let _ = r.reply.send(Err(OpError::new(Code::Stale, "no event is open")));
                }
                Kind::Finish(_) => {
                    let _ = r.reply.send(Ok(Value::Null));
                }
            }
        }
    }

    fn transaction(&self, s: &mut Session, begin: Request, rx: &Receiver<Request>, deferred: &mut VecDeque<Request>) {
        if begin.sent.elapsed() > OP_TIMEOUT {
            let _ = begin.reply.send(Err(OpError::new(Code::Stale, "the request expired")));
            return;
        }
        let Some((version, plugin_name, own_panels)) = self.plugin_info(&begin.plugin) else {
            let _ = begin.reply.send(Err(OpError::denied("this plugin is not loaded")));
            return;
        };
        if self.grants.stopped(&begin.plugin, version) {
            let _ = begin.reply.send(Err(OpError::denied("this plugin has been turned off")));
            return;
        }
        let scope = Scope { comp: s.active_comp_id(), selection: s.state.selected_layers.clone(), plugin_name };
        let generation = self.grants.generation(&begin.plugin);
        let mark = s.begin_group();
        let selection: Vec<u64> = scope.selection.iter().map(|l| l.0).collect();
        if begin.reply.send(Ok(json!({"selection": selection}))).is_err() {
            s.finish_group(mark, "", false);
            return;
        }
        let deadline = Instant::now() + TX_TIMEOUT;
        let label = begin.label.clone();
        let mut ui: Vec<(u64, UiChange)> = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = match rx.recv_timeout(left) {
                Ok(r) => r,
                Err(_) => {
                    s.finish_group(mark, &label, false);
                    effectcraft_engine::ext::notify(s, format!("{}: the plugin took too long; its changes were undone", scope.plugin_name), true);
                    return;
                }
            };
            if r.key != begin.key {
                deferred.push_back(r);
                continue;
            }
            match r.kind {
                Kind::Call(op, args) => {
                    let out = match ops::run(s, &scope, &op, args, &own_panels) {
                        Ok(Outcome::Value(v)) => Ok(v),
                        Ok(Outcome::Ui(change)) => {
                            let rev = self.next_rev.fetch_add(1, Ordering::Relaxed);
                            ui.push((rev, change));
                            Ok(json!({"rev": rev}))
                        }
                        Err(e) => Err(e),
                    };
                    let _ = r.reply.send(out);
                }
                Kind::Finish(commit) => {
                    let still = !self.grants.stopped(&begin.plugin, version) && self.grants.generation(&begin.plugin) == generation;
                    let commit = commit && still;
                    s.finish_group(mark, &label, commit);
                    if commit {
                        self.apply_ui(&begin, version, &scope.plugin_name, ui);
                    }
                    let _ = r.reply.send(Ok(Value::Null));
                    return;
                }
                Kind::Begin => {
                    let _ = r.reply.send(Err(OpError::new(Code::Stale, "an event is already open")));
                }
            }
        }
    }

    fn plugin_info(&self, id: &ExtensionId) -> Option<(Version, String, Vec<String>)> {
        let st = lock(&self.state);
        let p = st.loaded.iter().find(|p| &p.id == id)?;
        let panels = st.sessions.iter().filter(|c| &c.plugin == id && c.kind == ContributionKind::Panel).map(|c| c.contribution.clone()).collect();
        Some((p.version, p.name.clone(), panels))
    }

    fn apply_ui(&self, by: &Request, version: Version, plugin_name: &str, changes: Vec<(u64, UiChange)>) {
        let plugin = &by.plugin;
        let mut st = lock(&self.state);
        for (rev, c) in changes {
            match c {
                UiChange::Panel { panel, tree } => {
                    if let Some(sf) = st.surfaces.get_mut(&format!("{plugin}/{panel}")) {
                        sf.tree = Some(tree);
                        sf.rev = rev;
                    }
                }
                UiChange::Window { id, title, tree } => {
                    let key = format!("{plugin}/{id}");
                    let windows = st.surfaces.iter().filter(|(k, s)| s.window && &s.plugin == plugin && **k != key).count();
                    if windows >= MAX_WINDOWS {
                        continue;
                    }
                    let worker =
                        st.sessions.iter().find(|c| &c.plugin == plugin && c.contribution == by.contribution).map(|c| c.worker.clone()).unwrap_or_default();
                    st.surfaces.insert(
                        key,
                        Surface {
                            title,
                            tree: Some(tree),
                            rev,
                            window: true,
                            open: true,
                            worker,
                            contribution: by.contribution.clone(),
                            declared: by.declared.clone(),
                            plugin: plugin.clone(),
                            version,
                            plugin_name: plugin_name.to_string(),
                        },
                    );
                }
            }
        }
    }
}

fn local(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut b = Vec::new();
    f.take(MAX_DATA_BYTES + 1).read_to_end(&mut b).map_err(|e| e.to_string())?;
    if b.len() as u64 > MAX_DATA_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    Ok(b)
}

/// The worker loop: owns one sandboxed session (or native helper) and runs its events in order.
fn worker_main(name: &str, program: Program, rx: Receiver<Job>, api: Arc<dyn AppApi>, grants: Arc<dyn Grants>, log: LogFn) {
    enum Runner {
        Wasm(WasmSession),
        Native(craft_ext_host::native::Helper),
    }
    let started = match program {
        Program::Wasm(p) => WasmSession::new(&p, api, grants, log).map(Runner::Wasm),
        Program::Native { plugin, version, exe, work } => {
            craft_ext_host::native::Helper::start(&plugin, version, &exe, &work, api, grants, log).map(Runner::Native)
        }
    };
    let mut runner = match started {
        Ok(r) => r,
        Err(e) => {
            log::warn!("plugin {name}: {e}");
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Event { done: Some(d), .. } => {
                        let _ = d.send(Err(e.to_string()));
                    }
                    Job::Event { .. } => {}
                    Job::Stop => break,
                }
            }
            return;
        }
    };
    while let Ok(job) = rx.recv() {
        match job {
            Job::Event { inv, event, done } => {
                let r = match &mut runner {
                    Runner::Wasm(s) => s.dispatch(inv, &event),
                    Runner::Native(h) => h.dispatch(inv, &event),
                };
                if let Err(e) = &r {
                    log::warn!("plugin {name}: {e}");
                }
                if let Some(d) = done {
                    let _ = d.send(r.map_err(|e| e.to_string()));
                }
            }
            Job::Stop => break,
        }
    }
    if let Runner::Native(h) = &mut runner {
        h.kill();
    }
}

fn status_text(s: &Status) -> String {
    match s {
        Status::Ready { version } => format!("ready ({version})"),
        Status::NeedsEnable { version } => format!("installed ({version}) but turned off or missing permissions"),
        Status::NeedsOtherVersion { installed, wanted } => format!("needs version {wanted} (installed: {installed})"),
        Status::Downloadable => "not installed (listed in the catalogue)".into(),
        Status::Unavailable { reason } => format!("unavailable: {reason}"),
    }
}

/// Plugin ids of the plugin effects used in `project`.
fn used_plugins(project: &Project) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, c) in project.comps() {
        for l in &c.layers {
            for g in l.effects().into_iter().flat_map(|fx| fx.groups()) {
                if let effectcraft_project::GroupKind::Effect { effect } = &g.kind
                    && effectcraft_engine::ext::is_plugin_effect(effect)
                    && let Some(plugin) = effect.split('/').next()
                {
                    out.insert(plugin.to_string());
                }
            }
        }
    }
    out
}

impl Runtime {
    fn apply_preset(&self, s: &mut Session, p: &Preset) -> Result<Value, EngineError> {
        let mark = s.begin_group();
        let label = format!("{} · {}", p.label, p.plugin_name);
        let run = |s: &mut Session| -> Result<(), EngineError> {
            for (effect, params) in &p.effects {
                if effectcraft_engine::effects::find(effect).is_none() {
                    return Err(EngineError::Other(format!("preset {}: the effect {effect} is not available (is its plugin installed and enabled?)", p.label)));
                }
                let r = s.execute("effect.apply", json!({"effect": effect}))?;
                let layers = s.state.selected_layers.clone();
                let paths: Vec<String> = r["paths"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
                for (layer, path) in layers.iter().zip(&paths) {
                    for (k, v) in params.iter().take(64) {
                        if local(k) {
                            s.execute("prop.set", json!({"layer": layer.0, "path": format!("{path}/{k}"), "value": v}))?;
                        }
                    }
                }
            }
            Ok(())
        };
        match run(s) {
            Ok(()) => {
                s.finish_group(mark, &label, true);
                Ok(json!({"preset": p.id}))
            }
            Err(e) => {
                s.finish_group(mark, &label, false);
                Err(e)
            }
        }
    }

    fn app_events(&self, s: &mut Session) {
        let mut events: Vec<Value> = Vec::new();
        {
            let mut seen = lock(&self.seen);
            if seen.selection != s.state.selected_layers {
                seen.selection = s.state.selected_layers.clone();
                events
                    .push(json!({"event": "app", "name": "selection.changed", "payload": {"layers": seen.selection.iter().map(|l| l.0).collect::<Vec<_>>()}}));
            }
            let t = s.time().seconds();
            if seen.time != Some(t) && seen.time_sent.is_none_or(|at| at.elapsed() >= Duration::from_millis(250)) {
                seen.time = Some(t);
                seen.time_sent = Some(Instant::now());
                events.push(json!({"event": "app", "name": "time.changed", "payload": {"time": t}}));
            }
            let rendering = s.is_rendering();
            if seen.rendering && !rendering {
                events.push(json!({"event": "app", "name": "render.finished", "payload": {}}));
            }
            seen.rendering = rendering;
            if let Some(path) = seen.opened.take() {
                events.push(json!({"event": "app", "name": "document.opened", "payload": {"path": path}}));
            }
        }
        if events.is_empty() {
            return;
        }
        let hooks: Vec<SessionContribution> = {
            let st = lock(&self.state);
            st.sessions.iter().filter(|c| c.kind == ContributionKind::EventHook).cloned().collect()
        };
        for c in hooks {
            // Each hook needs `app.events`, declared by it and granted to the package.
            if !c.declared.iter().any(|d| d.covers(&Permission::AppEvents)) || !self.grants.allowed(&c.plugin, &Permission::AppEvents) {
                continue;
            }
            let label = format!("{} · {}", c.label, c.plugin_name);
            for e in &events {
                let inv = self.invocation(&c.plugin, c.version, &c.contribution, &label, c.declared.clone());
                let worker = c.worker.clone();
                // Never blocks: a full queue drops the event.
                let _ = self.deliver(s, &worker, inv, e.clone(), false);
            }
        }
    }
}

impl PluginRuntime for Runtime {
    fn commands(&self) -> Vec<PluginCommand> {
        let st = lock(&self.state);
        let mut v: Vec<PluginCommand> = st
            .sessions
            .iter()
            .filter(|c| c.kind == ContributionKind::Command)
            .map(|c| PluginCommand {
                id: format!("plugin.{}.{}", c.plugin, c.contribution),
                label: c.label.clone(),
                plugin: c.plugin_name.clone(),
                category: c.category.clone(),
            })
            .collect();
        v.extend(st.presets.iter().map(|p| PluginCommand {
            id: p.command.clone(),
            label: p.label.clone(),
            plugin: p.plugin_name.clone(),
            category: Some("Presets".into()),
        }));
        v
    }

    fn run_command(&self, s: &mut Session, id: &str, _params: &Value) -> effectcraft_engine::Result<Value> {
        let (preset, session) = {
            let st = lock(&self.state);
            let preset = st.presets.iter().find(|p| p.command == id).cloned();
            let session = st.sessions.iter().find(|c| c.kind == ContributionKind::Command && format!("plugin.{}.{}", c.plugin, c.contribution) == id).cloned();
            (preset, session)
        };
        if let Some(p) = preset {
            return self.apply_preset(s, &p);
        }
        let c = session.ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        let inv = self.invocation(&c.plugin, c.version, &c.contribution, &format!("{} · {}", c.label, c.plugin_name), c.declared.clone());
        let wait = !self.interactive.load(Ordering::Relaxed);
        self.deliver(s, &c.worker, inv, json!({"event": "command", "id": c.contribution}), wait)
            .map_err(|e| EngineError::Other(format!("{}: {e}", c.plugin_name)))?;
        Ok(json!({"plugin": c.plugin.as_str(), "contribution": c.contribution, "finished": wait}))
    }

    fn pump(&self, s: &mut Session) {
        self.interactive.store(true, Ordering::Relaxed);
        // Enable, disable, grant, revoke, update and kill-list changes apply without a restart.
        let changes = self.grants.changes();
        let stale = lock(&self.state).store_changes != changes;
        if stale {
            self.unload("This effect's plugin was turned off or changed.");
            self.load(s);
        }
        let failures: Vec<String> = {
            let r = lock(&self.results);
            std::iter::from_fn(|| r.1.try_recv().ok()).filter_map(Result::err).collect()
        };
        for e in failures.into_iter().take(4) {
            effectcraft_engine::ext::notify(s, format!("Plugin: {e}"), true);
        }
        self.serve(s, Duration::ZERO);
        self.app_events(s);
    }

    fn surfaces(&self) -> Vec<PluginSurface> {
        let st = lock(&self.state);
        st.surfaces
            .iter()
            .map(|(k, v)| PluginSurface {
                key: k.clone(),
                title: format!("{} — {}", v.title, v.plugin_name),
                tree: v.tree.clone(),
                rev: v.rev,
                window: v.window,
                open: v.open,
            })
            .collect()
    }

    fn ui_event(&self, s: &mut Session, key: &str, rev: u64, widget: &str, value: Value) {
        let target = {
            let st = lock(&self.state);
            st.surfaces.get(key).filter(|sf| sf.rev == rev).map(|sf| {
                (sf.worker.clone(), sf.plugin.clone(), sf.version, format!("{} · {}", sf.title, sf.plugin_name), sf.contribution.clone(), sf.declared.clone())
            })
        };
        // Stale revision or unknown surface: dropped (R12).
        let Some((worker, plugin, version, label, contribution, declared)) = target else { return };
        let surface = key.split_once('/').map_or(key, |(_, local)| local);
        let widget: String = widget.chars().take(64).collect();
        let event = json!({"event": "ui", "surface": surface, "rev": rev, "widget": widget, "value": value});
        let inv = self.invocation(&plugin, version, &contribution, &label, declared);
        let wait = !self.interactive.load(Ordering::Relaxed);
        if let Err(e) = self.deliver(s, &worker, inv, event, wait) {
            effectcraft_engine::ext::notify(s, format!("Plugin: {e}"), true);
        }
    }

    fn set_open(&self, key: &str, open: bool) {
        let mut st = lock(&self.state);
        match st.surfaces.get_mut(key) {
            Some(sf) if sf.window && !open => {
                st.surfaces.remove(key);
            }
            Some(sf) => sf.open = open,
            None => {}
        }
    }

    fn project_plugins(&self, project: &Project) -> Vec<effectcraft_project::PluginRef> {
        let st = lock(&self.state);
        used_plugins(project)
            .into_iter()
            .filter_map(|id| {
                if let Some(p) = st.loaded.iter().find(|p| p.id.as_str() == id) {
                    Some(effectcraft_project::PluginRef {
                        id,
                        version: p.version.to_string(),
                        sha256: p.sha256.clone(),
                        source: match p.source {
                            Source::Catalogue => "catalogue".into(),
                            Source::Local => "local".into(),
                        },
                        name: p.name.clone(),
                    })
                } else {
                    // Missing here: keep what the project said, so saving never loses it.
                    project.plugins.iter().find(|r| r.id == id).cloned()
                }
            })
            .collect()
    }

    fn project_opened(&self, s: &mut Session) {
        lock(&self.seen).opened = Some(s.path.clone().unwrap_or_default());
        let refs: Vec<PluginRef> = s
            .project
            .plugins
            .iter()
            .take(256)
            .filter_map(|r| {
                Some(PluginRef {
                    id: ExtensionId::new(&r.id).ok()?,
                    version: Version::parse(&r.version).ok()?,
                    sha256: r.sha256.clone().filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())),
                    source: if r.source == "catalogue" { Source::Catalogue } else { Source::Local },
                    name: r.name.chars().filter(|c| !c.is_control()).take(64).collect(),
                })
            })
            .collect();
        if refs.is_empty() {
            return;
        }
        let statuses = match self.manager().check_project(&refs) {
            Ok(st) => st,
            Err(e) => {
                log::warn!("plugins: cannot check the project's plugins: {e}");
                return;
            }
        };
        let Some(line) = craft_ext_core::project::summary(&statuses) else { return };
        effectcraft_engine::ext::notify(s, line.clone(), false);
        if !self.interactive.load(Ordering::Relaxed) {
            eprintln!("{line}");
            for (r, st) in statuses.iter().filter(|(_, st)| !st.ready()) {
                eprintln!("  {} {} ({}): {}", r.id, r.version, r.name, status_text(st));
                match st {
                    Status::Downloadable | Status::NeedsOtherVersion { .. } => {
                        eprintln!("    effectcraft-cli plugin install {} --version ={}", r.id, r.version)
                    }
                    Status::NeedsEnable { .. } => eprintln!("    effectcraft-cli plugin setup {} --all", r.id),
                    _ => {}
                }
            }
        }
        *lock(&self.prompt) = Some(statuses);
    }

    fn describe(&self) -> Value {
        let st = lock(&self.state);
        let loaded: Vec<Value> = st
            .loaded
            .iter()
            .map(|p| json!({"id": p.id.as_str(), "version": p.version.to_string(), "name": p.name, "contributions": p.enabled.iter().collect::<Vec<_>>()}))
            .collect();
        json!({"safeMode": self.safe, "plugins": loaded, "effects": st.effects})
    }

    fn resources(&self) -> Value {
        let st = lock(&self.state);
        let presets: Vec<Value> = st
            .presets
            .iter()
            .map(|p| {
                let effects: Vec<Value> = p.effects.iter().map(|(e, params)| json!({"effect": e, "params": params})).collect();
                json!({"plugin": p.plugin.as_str(), "contribution": p.contribution, "id": p.id, "label": p.label, "command": p.command, "effects": effects})
            })
            .collect();
        let luts: Vec<Value> = st
            .luts
            .iter()
            .map(|l| json!({"plugin": l.plugin.as_str(), "contribution": l.contribution, "label": l.label, "path": l.path.to_string_lossy()}))
            .collect();
        json!({"presets": presets, "luts": luts})
    }
}
