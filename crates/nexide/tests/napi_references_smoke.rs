//! End-to-end test for `napi_create_reference` / `_ref` / `_unref` /
//! `napi_get_reference_value` / `napi_delete_reference`.
//!
//! Round-trip: addon stashes a JS object via a ref, returns its
//! `id`. A second call resolves the ref and reads back a property.
//! Refcount manipulation is exercised via `bumpAndPeek`.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

typedef enum { napi_ok = 0 } napi_status;
typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;
typedef struct { void* p; } napi_callback_info;
typedef struct { void* p; } napi_ref;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);

extern napi_status napi_create_int32(napi_env, int, napi_value*);
extern napi_status napi_get_value_int32(napi_env, napi_value, int*);
extern napi_status napi_create_function(napi_env, const char*, size_t,
                                        napi_callback, void*, napi_value*);
extern napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t*,
                                    napi_value*, napi_value*, void**);
extern napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);
extern napi_status napi_get_named_property(napi_env, napi_value, const char*, napi_value*);
extern napi_status napi_create_reference(napi_env, napi_value, unsigned int, napi_ref*);
extern napi_status napi_get_reference_value(napi_env, napi_ref, napi_value*);
extern napi_status napi_delete_reference(napi_env, napi_ref);
extern napi_status napi_reference_ref(napi_env, napi_ref, unsigned int*);
extern napi_status napi_reference_unref(napi_env, napi_ref, unsigned int*);

/* Single-slot ref store (test only). */
static napi_ref g_ref;
static int g_has_ref = 0;

static napi_value stash_impl(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  if (g_has_ref) napi_delete_reference(env, g_ref);
  napi_create_reference(env, argv[0], 1, &g_ref);
  g_has_ref = 1;
  napi_value out;
  napi_create_int32(env, 1, &out);
  return out;
}

static napi_value peek_impl(napi_env env, napi_callback_info info) {
  napi_value v;
  napi_get_reference_value(env, g_ref, &v);
  napi_value n;
  napi_get_named_property(env, v, "answer", &n);
  int x = 0;
  napi_get_value_int32(env, n, &x);
  napi_value out;
  napi_create_int32(env, x, &out);
  return out;
}

static napi_value bump_count_impl(napi_env env, napi_callback_info info) {
  unsigned int after_inc = 0;
  napi_reference_ref(env, g_ref, &after_inc);
  unsigned int after_dec = 0;
  napi_reference_unref(env, g_ref, &after_dec);
  /* return after_inc * 1000 + after_dec */
  napi_value out;
  napi_create_int32(env, (int)(after_inc * 1000 + after_dec), &out);
  return out;
}

static napi_value drop_impl(napi_env env, napi_callback_info info) {
  napi_delete_reference(env, g_ref);
  g_has_ref = 0;
  napi_value out;
  napi_create_int32(env, 0, &out);
  return out;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value fn;
  napi_create_function(env, "stash", (size_t)-1, stash_impl, NULL, &fn);
  napi_set_named_property(env, exports, "stash", fn);
  napi_create_function(env, "peek", (size_t)-1, peek_impl, NULL, &fn);
  napi_set_named_property(env, exports, "peek", fn);
  napi_create_function(env, "bumpAndPeek", (size_t)-1, bump_count_impl, NULL, &fn);
  napi_set_named_property(env, exports, "bumpAndPeek", fn);
  napi_create_function(env, "drop", (size_t)-1, drop_impl, NULL, &fn);
  napi_set_named_property(env, exports, "drop", fn);
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
async fn references_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});
        addon.stash({{ answer: 42 }});
        const peeked = addon.peek();
        if (peeked !== 42) throw new Error("peek=" + peeked);
        const refStats = addon.bumpAndPeek();
        if (refStats !== 2 * 1000 + 1) throw new Error("ref counts wrong: " + refStats);
        addon.drop();
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
        panic!("references test failed: {err}");
    }
}
