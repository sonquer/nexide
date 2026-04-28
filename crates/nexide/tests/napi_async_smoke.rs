//! End-to-end test: an addon schedules `napi_create_async_work` /
//! `napi_queue_async_work`, the execute callback runs on the blocking
//! pool, and the complete callback invokes a JS callback on the main
//! thread with the result.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>
#include <stdlib.h>
#include <unistd.h>

typedef enum { napi_ok = 0, napi_cancelled = 11 } napi_status;
typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;
typedef struct { void* p; } napi_callback_info;
typedef struct { void* p; } napi_async_work;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);
typedef void (*napi_async_execute_callback)(napi_env, void*);
typedef void (*napi_async_complete_callback)(napi_env, napi_status, void*);

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
extern napi_status napi_create_string_utf8(napi_env, const char*, size_t, napi_value*);
extern napi_status napi_create_async_work(napi_env, napi_value, napi_value,
                                          napi_async_execute_callback,
                                          napi_async_complete_callback,
                                          void*, napi_async_work*);
extern napi_status napi_queue_async_work(napi_env, napi_async_work);
extern napi_status napi_delete_async_work(napi_env, napi_async_work);

typedef struct {
  napi_async_work work;
  int input;
  int output;
} ctx_t;

static void exec_cb(napi_env env, void* data) {
  ctx_t* c = (ctx_t*)data;
  usleep(20000);
  c->output = c->input * c->input;
}

static void complete_cb(napi_env env, napi_status status, void* data) {
  ctx_t* c = (ctx_t*)data;
  napi_value global;
  napi_get_global(env, &global);
  napi_value cb;
  extern napi_status napi_get_named_property(napi_env, napi_value, const char*, napi_value*);
  napi_get_named_property(env, global, "__asyncCb", &cb);
  napi_value arg;
  napi_create_int32(env, c->output, &arg);
  napi_value res;
  napi_call_function(env, global, cb, 1, &arg, &res);

  napi_delete_async_work(env, c->work);
  free(c);
}

static napi_value run_async_impl(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);

  /* stash arg[1] on globalThis.__asyncCb */
  napi_value global;
  napi_get_global(env, &global);
  napi_set_named_property(env, global, "__asyncCb", argv[1]);

  ctx_t* c = (ctx_t*)calloc(1, sizeof(ctx_t));
  napi_get_value_int32(env, argv[0], &c->input);

  napi_value name;
  napi_create_string_utf8(env, "smoke", (size_t)-1, &name);
  napi_create_async_work(env, (napi_value){0}, name, exec_cb, complete_cb, c, &c->work);
  napi_queue_async_work(env, c->work);

  napi_value undef;
  napi_create_int32(env, 0, &undef);
  return undef;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value fn;
  napi_create_function(env, "runAsync", (size_t)-1, run_async_impl, NULL, &fn);
  napi_set_named_property(env, exports, "runAsync", fn);
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

    for _ in 0..200 {
        engine.pump_once();
        // Probe throws while result is missing; ok means done.
        let probe = engine.execute(
            "probe",
            "if (globalThis.__result === undefined) throw 0; \
             if (globalThis.__result !== 49) throw new Error('bad value');",
        );
        if probe.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err("timeout waiting for async work".to_string())
}

#[tokio::test(flavor = "current_thread")]
async fn async_work_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});
        addon.runAsync(7, (value) => {{ globalThis.__result = value; }});
        "#
    );
    std::fs::write(&entry, body).expect("write entry");

    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module_with_pump(&dir_path, &entry).await })
        .await;
    drop(dir);
    match result {
        Ok(()) => {}
        Err(err) => panic!("async work test failed: {err}"),
    }
}
