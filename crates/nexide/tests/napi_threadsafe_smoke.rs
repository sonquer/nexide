//! End-to-end test for threadsafe-functions: addon spawns a POSIX
//! thread that calls `napi_call_threadsafe_function` from outside the
//! V8 thread. The call_js_cb runs on the JS thread, invoking a JS
//! callback the test waits for.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>
#include <stdlib.h>
#include <pthread.h>
#include <unistd.h>

typedef enum { napi_ok = 0, napi_closing = 17 } napi_status;
typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;
typedef struct { void* p; } napi_callback_info;
typedef struct { void* p; } napi_threadsafe_function;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);
typedef void (*napi_threadsafe_function_call_js)(napi_env, napi_value, void*, void*);
typedef void (*napi_finalize)(napi_env, void*, void*);

extern napi_status napi_create_int32(napi_env, int, napi_value*);
extern napi_status napi_get_value_int32(napi_env, napi_value, int*);
extern napi_status napi_create_function(napi_env, const char*, size_t,
                                        napi_callback, void*, napi_value*);
extern napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t*,
                                    napi_value*, napi_value*, void**);
extern napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);
extern napi_status napi_get_global(napi_env, napi_value*);
extern napi_status napi_call_function(napi_env, napi_value, napi_value, size_t,
                                      const napi_value*, napi_value*);

extern napi_status napi_create_threadsafe_function(
    napi_env, napi_value, napi_value, napi_value, size_t, size_t, void*,
    napi_finalize, void*, napi_threadsafe_function_call_js,
    napi_threadsafe_function*);
extern napi_status napi_call_threadsafe_function(
    napi_threadsafe_function, void*, int);
extern napi_status napi_release_threadsafe_function(
    napi_threadsafe_function, int);

static napi_threadsafe_function g_tsfn;

/* runs on JS thread, builds the args and calls the captured func */
static void call_js_cb(napi_env env, napi_value js_callback, void* context, void* data) {
  napi_value global;
  napi_get_global(env, &global);
  napi_value arg;
  napi_create_int32(env, (int)(long)data, &arg);
  napi_value res;
  napi_call_function(env, global, js_callback, 1, &arg, &res);
}

/* worker thread: posts 3 calls then releases the tsfn */
static void* worker(void* arg) {
  for (long i = 1; i <= 3; ++i) {
    usleep(10000);
    napi_call_threadsafe_function(g_tsfn, (void*)(long)(i * 7), 0);
  }
  napi_release_threadsafe_function(g_tsfn, 0);
  return NULL;
}

static napi_value start_impl(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);

  napi_value name;
  napi_create_int32(env, 0, &name);  /* placeholder */
  napi_create_threadsafe_function(env, argv[0], (napi_value){0}, name,
                                  0, 1, NULL, NULL, NULL, call_js_cb, &g_tsfn);
  pthread_t tid;
  pthread_create(&tid, NULL, worker, NULL);
  pthread_detach(tid);

  napi_value out;
  napi_create_int32(env, 1, &out);
  return out;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value fn;
  napi_create_function(env, "start", (size_t)-1, start_impl, NULL, &fn);
  napi_set_named_property(env, exports, "start", fn);
  return exports;
}
"#;

fn build_addon(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("addon.c");
    std::fs::write(&src, ADDON_C).expect("write c source");

    let obj = dir.join("addon.o");
    let status = std::process::Command::new("cc")
        .args(["-fPIC", "-c", "-o"])
        .arg(&obj)
        .arg(&src)
        .status()
        .expect("invoke cc");
    assert!(status.success(), "cc compile failed");

    let out = dir.join("addon.node");
    let mut link = std::process::Command::new("cc");
    if cfg!(target_os = "macos") {
        link.args([
            "-dynamiclib",
            "-undefined",
            "dynamic_lookup",
            "-Wl,-flat_namespace",
        ]);
    } else {
        link.arg("-shared");
    }
    let status = link
        .arg("-o")
        .arg(&out)
        .arg(&obj)
        .arg("-lpthread")
        .status()
        .expect("invoke cc link");
    assert!(status.success(), "cc link failed");
    out
}

async fn run_module_with_pump(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(std::iter::empty::<(String, String)>()));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new().with_cjs(resolver).with_process(process);
    let mut engine = V8Engine::boot_with(entry, ctx)
        .await
        .map_err(|e| e.to_string())?;

    for _ in 0..400 {
        engine.pump_once();
        let probe = engine.execute(
            "probe",
            "if (globalThis.__sum !== 7 + 14 + 21) throw 0;",
        );
        if probe.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err("timeout waiting for tsfn calls".to_string())
}

#[tokio::test(flavor = "current_thread")]
async fn threadsafe_function_dispatches_from_worker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});
        globalThis.__sum = 0;
        addon.start((value) => {{ globalThis.__sum += value; }});
        "#
    );
    std::fs::write(&entry, body).expect("write entry");

    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module_with_pump(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("threadsafe-function test failed: {err}");
    }
}
