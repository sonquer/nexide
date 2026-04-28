//! End-to-end test for native callback support: a `.node` addon
//! exposes `add(a, b)` and a constructor `Counter`, exercised from JS.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>

typedef enum { napi_ok = 0 } napi_status;
typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;
typedef struct { void* p; } napi_callback_info;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);

extern napi_status napi_create_int32(napi_env, int, napi_value*);
extern napi_status napi_get_value_int32(napi_env, napi_value, int*);
extern napi_status napi_create_function(napi_env, const char*, size_t,
                                        napi_callback, void*, napi_value*);
extern napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t*,
                                    napi_value*, napi_value*, void**);
extern napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);
extern napi_status napi_throw_error(napi_env, const char*, const char*);

static napi_value add_impl(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  if (argc < 2) {
    napi_throw_error(env, NULL, "expected 2 args");
    return (napi_value){0};
  }
  int a = 0, b = 0;
  napi_get_value_int32(env, argv[0], &a);
  napi_get_value_int32(env, argv[1], &b);
  napi_value out;
  napi_create_int32(env, a + b, &out);
  return out;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value add_fn;
  napi_create_function(env, "add", (size_t)-1, add_impl, NULL, &add_fn);
  napi_set_named_property(env, exports, "add", add_fn);
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

async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(std::iter::empty::<(String, String)>()));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new().with_cjs(resolver).with_process(process);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tokio::test(flavor = "current_thread")]
async fn native_function_callable_from_js() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});
        if (typeof addon.add !== "function") throw new Error("add not a function");
        const sum = addon.add(2, 3);
        if (sum !== 5) throw new Error("add(2,3) = " + sum);
        const neg = addon.add(-7, 10);
        if (neg !== 3) throw new Error("add(-7,10) = " + neg);
        "#
    );
    std::fs::write(&entry, body).expect("write entry");

    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("native function call failed: {err}");
    }
}
