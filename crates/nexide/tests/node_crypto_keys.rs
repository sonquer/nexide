//! Integration tests for the `node:crypto` key/sign/verify/ecdh/hkdf surface.

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

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

async fn assert_passes(body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, body).expect("write entry");
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rsa_generate_sign_verify_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('rsa', { modulusLength: 2048 });\n\
         const data = Buffer.from('hello rsa');\n\
         const sig = c.sign('sha256', data, privateKey);\n\
         if (!c.verify('sha256', data, publicKey, sig)) throw new Error('rsa verify failed');\n\
         if (c.verify('sha256', Buffer.from('tampered'), publicKey, sig)) throw new Error('verify must fail on tampered data');\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ec_p256_sign_verify_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('ec', { namedCurve: 'P-256' });\n\
         const data = Buffer.from('hello ec');\n\
         const sig = c.sign('sha256', data, privateKey);\n\
         if (!c.verify('sha256', data, publicKey, sig)) throw new Error('ec verify failed');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ed25519_sign_verify_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('ed25519');\n\
         const data = Buffer.from('hello ed25519');\n\
         const sig = c.sign(null, data, privateKey);\n\
         if (!c.verify(null, data, publicKey, sig)) throw new Error('ed25519 verify failed');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rsa_oaep_encrypt_decrypt_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('rsa', { modulusLength: 2048 });\n\
         const msg = Buffer.from('secret oaep payload');\n\
         const ct = c.publicEncrypt({ key: publicKey, padding: c.constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256' }, msg);\n\
         const pt = c.privateDecrypt({ key: privateKey, padding: c.constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256' }, ct);\n\
         if (pt.toString() !== msg.toString()) throw new Error('oaep mismatch: ' + pt.toString());\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn rsa_pkcs1_encrypt_decrypt_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('rsa', { modulusLength: 2048 });\n\
         const msg = Buffer.from('pkcs1 v1.5');\n\
         const ct = c.publicEncrypt({ key: publicKey, padding: c.constants.RSA_PKCS1_PADDING }, msg);\n\
         const pt = c.privateDecrypt({ key: privateKey, padding: c.constants.RSA_PKCS1_PADDING }, ct);\n\
         if (pt.toString() !== msg.toString()) throw new Error('pkcs1 mismatch: ' + pt.toString());\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ecdh_p256_two_party_shared_secret() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const a = c.createECDH('prime256v1'); a.generateKeys();\n\
         const b = c.createECDH('prime256v1'); b.generateKeys();\n\
         const s1 = a.computeSecret(b.getPublicKey()).toString('hex');\n\
         const s2 = b.computeSecret(a.getPublicKey()).toString('hex');\n\
         if (s1 !== s2) throw new Error('ecdh secrets differ');\n\
         if (s1.length !== 64) throw new Error('expected 32-byte secret');\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn x25519_diffie_hellman_two_party_shared_secret() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const a = c.generateKeyPairSync('x25519');\n\
         const b = c.generateKeyPairSync('x25519');\n\
         const s1 = c.diffieHellman({ privateKey: a.privateKey, publicKey: b.publicKey }).toString('hex');\n\
         const s2 = c.diffieHellman({ privateKey: b.privateKey, publicKey: a.publicKey }).toString('hex');\n\
         if (s1 !== s2) throw new Error('x25519 secrets differ');\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn hkdf_sha256_rfc5869_test_case_1() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const ikm = Buffer.from('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b', 'hex');\n\
         const salt = Buffer.from('000102030405060708090a0b0c', 'hex');\n\
         const info = Buffer.from('f0f1f2f3f4f5f6f7f8f9', 'hex');\n\
         const out = Buffer.from(c.hkdfSync('sha256', ikm, salt, info, 42));\n\
         const expected = '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865';\n\
         if (out.toString('hex') !== expected) throw new Error('hkdf mismatch: ' + out.toString('hex'));\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn keyobject_pem_export_reimport_roundtrip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { publicKey, privateKey } = c.generateKeyPairSync('ec', { namedCurve: 'P-256' });\n\
         const privPem = privateKey.export({ type: 'pkcs8', format: 'pem' });\n\
         const pubPem = publicKey.export({ type: 'spki', format: 'pem' });\n\
         if (typeof privPem !== 'string' || !privPem.includes('PRIVATE KEY')) throw new Error('bad priv pem');\n\
         if (typeof pubPem !== 'string' || !pubPem.includes('PUBLIC KEY')) throw new Error('bad pub pem');\n\
         const priv2 = c.createPrivateKey(privPem);\n\
         const pub2 = c.createPublicKey(pubPem);\n\
         const data = Buffer.from('roundtrip');\n\
         const sig = c.sign('sha256', data, priv2);\n\
         if (!c.verify('sha256', data, pub2, sig)) throw new Error('roundtrip verify failed');\n",
    ).await;
}

#[tokio::test(flavor = "current_thread")]
async fn keyobject_jwk_export_rsa_private_has_crt_fields() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const { privateKey } = c.generateKeyPairSync('rsa', { modulusLength: 2048 });\n\
         const jwk = privateKey.export({ format: 'jwk' });\n\
         for (const k of ['kty','n','e','d','p','q','dp','dq','qi']) {\n\
           if (typeof jwk[k] !== 'string') throw new Error('missing jwk field ' + k);\n\
         }\n\
         if (jwk.kty !== 'RSA') throw new Error('expected kty=RSA');\n",
    )
    .await;
}
