//! An embedded JavaScript engine (QuickJS, through `rquickjs`) for running a
//! JavaScript tool inside the binary instead of under `node`.
//!
//! The engine runs on its own thread, which owns the runtime; callers send it
//! one string and get one string back. It loads global scripts (a library
//! that defines a global, the way `typescript.js` defines `ts`), then
//! evaluates an entry module that may import the other modules it was given
//! and the `node:*` modules in [`NODE_MODULES`]. Those are stand-ins: only
//! POSIX `path` works, and every other member throws an error starting with
//! [`UNSUPPORTED`], which a caller can tell apart from the tool's own throw.
//!
//! Nothing here is specific to a language or a tool; what to load and what
//! to call comes in [`Setup`].

use std::sync::mpsc;

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{Context, Ctx, Function, Module, Runtime};
use tokio::sync::oneshot;

/// The prefix of the message a stand-in `node:*` member throws.
pub const UNSUPPORTED: &str = "EMBEDDED-UNSUPPORTED";

/// The `node:*` modules an entry can import, with the named exports each one
/// stands in for. `node:path` is a port of Node's POSIX functions; the
/// others' members throw [`UNSUPPORTED`] when called.
pub const NODE_MODULES: &[(&str, &[&str])] = &[
    ("node:path", &["posix", "normalize", "join", "dirname"]),
    (
        "node:fs",
        &["readFileSync", "realpathSync", "writeFileSync"],
    ),
    ("node:url", &["fileURLToPath"]),
    ("node:zlib", &["crc32"]),
    ("node:child_process", &["execFileSync"]),
    ("node:module", &["createRequire"]),
];

const NODE_PATH: &str = include_str!("js/node_path.mjs");

/// The native stack the engine's thread reserves. QuickJS recurses on it, so
/// it must stay well above any limit [`Stack`] sets.
const THREAD_STACK: usize = 512 << 20;

/// The JavaScript stack limit.
pub enum Stack {
    /// Bytes.
    Fixed(usize),
    /// The limit at which the global function `probe(n)` first returns
    /// false at `n == target`: the engine measures where it fails under a
    /// reference limit and scales, so a tool's recursion runs out where it
    /// runs out under another engine with a different frame size.
    Calibrated { probe: String, target: u32 },
}

/// What the engine loads, and the global function each call goes to.
pub struct Setup {
    /// `(name, source)`, evaluated as classic scripts in order.
    pub scripts: Vec<(String, String)>,
    /// `(name, source)` an entry can import, by name or relative path.
    pub modules: Vec<(String, String)>,
    /// `(name, source)` of the module evaluated after the scripts.
    pub entry: (String, String),
    /// A JavaScript expression whose string value [`JsEngine::start`]
    /// returns once everything loaded.
    pub hello: String,
    /// The global function a call goes to: one string in, one string out.
    pub call: String,
    pub stack: Stack,
}

type Job = (String, oneshot::Sender<Option<String>>);

pub struct JsEngine {
    jobs: mpsc::Sender<Job>,
}

fn stub_module(name: &str, exports: &[&str]) -> String {
    let thrower =
        |what: &str| format!("() => {{ throw new Error(\"{UNSUPPORTED}: {name}.{what}\"); }}");
    let mut src: String = exports
        .iter()
        .map(|e| format!("export const {e} = {};\n", thrower(e)))
        .collect();
    src.push_str(&format!(
        "export default new Proxy({{}}, {{ get(_, p) {{ return {}; }} }});\n",
        thrower("${String(p)}").replace('"', "`")
    ));
    src
}

/// The message of the exception pending in `ctx`.
fn caught(ctx: &Ctx<'_>, e: &rquickjs::Error) -> String {
    let ex = ctx.catch();
    ex.as_exception()
        .and_then(rquickjs::Exception::message)
        .or_else(|| ex.as_string().and_then(|s| s.to_string().ok()))
        .unwrap_or_else(|| e.to_string())
}

fn load(rt: &Runtime, ctx: &Context, setup: &Setup) -> Result<String, String> {
    let mut resolver = BuiltinResolver::default();
    let mut loader = BuiltinLoader::default();
    for (name, exports) in NODE_MODULES {
        resolver.add_module(*name);
        let src = if *name == "node:path" {
            NODE_PATH.to_owned()
        } else {
            stub_module(name, exports)
        };
        loader.add_module(*name, src);
    }
    for (name, src) in &setup.modules {
        resolver.add_module(name.clone());
        loader.add_module(name.clone(), src.clone());
    }
    rt.set_loader(resolver, loader);
    rt.set_max_stack_size(THREAD_STACK / 2);
    ctx.with(|ctx| -> Result<String, String> {
        for (name, src) in &setup.scripts {
            ctx.eval::<(), _>(src.as_str())
                .map_err(|e| format!("{name}: {}", caught(&ctx, &e)))?;
        }
        let (name, src) = &setup.entry;
        Module::evaluate(ctx.clone(), name.as_str(), src.as_str())
            .and_then(|p| p.finish::<()>())
            .map_err(|e| format!("{name}: {}", caught(&ctx, &e)))?;
        ctx.eval::<String, _>(setup.hello.as_str())
            .map_err(|e| format!("hello: {}", caught(&ctx, &e)))
    })
}

/// Where `probe` first fails under `limit` bytes of stack.
fn threshold(rt: &Runtime, ctx: &Context, probe: &str, limit: usize) -> Option<u32> {
    rt.set_max_stack_size(limit);
    ctx.with(|ctx| {
        let f: Function = ctx.globals().get(probe).ok()?;
        let ok = |n: u32| f.call::<_, bool>((n,)).unwrap_or(false);
        let (mut lo, mut hi) = (1u32, 1u32 << 16);
        if !ok(lo) || ok(hi) {
            return None;
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(hi)
    })
}

fn set_stack(rt: &Runtime, ctx: &Context, stack: &Stack) -> Result<(), String> {
    match stack {
        Stack::Fixed(bytes) => rt.set_max_stack_size(*bytes),
        Stack::Calibrated { probe, target } => {
            const REFERENCE: usize = 4 << 20;
            let at = threshold(rt, ctx, probe, REFERENCE)
                .ok_or_else(|| format!("stack calibration: {probe} never failed"))?;
            let limit = REFERENCE / at as usize * *target as usize;
            if limit >= THREAD_STACK / 2 {
                return Err(format!("stack calibration: {limit} bytes is too deep"));
            }
            rt.set_max_stack_size(limit);
        }
    }
    Ok(())
}

fn serve(
    setup: &Setup,
    jobs: &mpsc::Receiver<Job>,
    ready: oneshot::Sender<Result<String, String>>,
) {
    let started = Runtime::new()
        .and_then(|rt| Context::full(&rt).map(|ctx| (rt, ctx)))
        .map_err(|e| e.to_string())
        .and_then(|(rt, ctx)| {
            let hello = load(&rt, &ctx, setup)?;
            set_stack(&rt, &ctx, &setup.stack)?;
            Ok((rt, ctx, hello))
        });
    let (_rt, ctx) = match started {
        Ok((rt, ctx, hello)) => {
            if ready.send(Ok(hello)).is_err() {
                return;
            }
            (rt, ctx)
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    for (line, reply) in jobs {
        let out = ctx.with(|ctx| {
            let f: Function = ctx.globals().get(setup.call.as_str()).ok()?;
            f.call::<_, String>((line,)).ok()
        });
        let _ = reply.send(out);
    }
}

impl JsEngine {
    /// Starts the engine's thread and loads `setup`. Returns the engine and
    /// the value of `setup.hello`.
    ///
    /// # Errors
    /// A script or module that does not load, an entry that throws, or a
    /// stack calibration that does not converge, with its message.
    pub async fn start(setup: Setup) -> Result<(Self, String), String> {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (ready, hello) = oneshot::channel();
        std::thread::Builder::new()
            .name("tribunal-js".to_owned())
            .stack_size(THREAD_STACK)
            .spawn(move || serve(&setup, &rx, ready))
            .map_err(|e| format!("cannot start the engine thread: {e}"))?;
        let hello = hello
            .await
            .map_err(|_| "the engine thread stopped".to_owned())??;
        Ok((Self { jobs }, hello))
    }

    /// Calls the setup's function with `line`. `None` when the function
    /// threw past its own handling or the engine stopped.
    pub async fn call(&self, line: String) -> Option<String> {
        let (reply, out) = oneshot::channel();
        self.jobs.send((line, reply)).ok()?;
        out.await.ok()?
    }
}
