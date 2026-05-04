"use strict";

// node:crypto - surface backed by `op_crypto_*` host calls.
//
// Hash/HMAC/cipher operations are exposed via accumulating JS shells
// (the host ops are one-shot) so that `update()` chunks stay in
// memory until `digest()` / `final()` issues a single op call.
//
// Supported ciphers (createCipheriv / createDecipheriv):
//   * `aes-128-cbc`, `aes-192-cbc`, `aes-256-cbc` (PKCS#7 padded)
//   * `aes-128-ctr`, `aes-256-ctr`
//   * `aes-256-gcm` (AEAD, with `setAAD` / `getAuthTag` / `setAuthTag`)
//   * `chacha20-poly1305` (AEAD, 12-byte nonce, 16-byte tag)
//
// KDFs: `pbkdf2`, `pbkdf2Sync`, `scrypt`, `scryptSync`.
//
// Sign/Verify (`createSign` / `createVerify`):
//   * RSASSA-PKCS1-v1_5 with SHA-256 / SHA-384 / SHA-512
//   * ECDSA on the P-256 curve (DER-encoded signatures)
//   * Ed25519

const ops = Nexide.core.ops;
const Buffer = globalThis.Buffer;

function concat(chunks) {
  let total = 0; for (const c of chunks) total += c.byteLength;
  const out = new Uint8Array(total);
  let off = 0; for (const c of chunks) { out.set(c, off); off += c.byteLength; }
  return out;
}

function asBytes(input, encoding) {
  if (input instanceof Uint8Array) return input;
  if (typeof input === "string") return Buffer.from(input, encoding || "utf8");
  if (input && typeof input === "object" && input.type === "Buffer" && Array.isArray(input.data)) {
    return Buffer.from(input.data);
  }
  throw new TypeError("expected Buffer/Uint8Array/string");
}

function normalizeAlgo(algo) {
  return String(algo).toLowerCase().replace(/-/g, "").replace(/_/g, "");
}

function normalizeDigestName(name) {
  const n = normalizeAlgo(name);
  switch (n) {
    case "sha1": return "sha1";
    case "sha256": return "sha256";
    case "sha384": return "sha384";
    case "sha512": return "sha512";
    case "md5": return "md5";
    default: return n;
  }
}

class Hash {
  constructor(algo) { this._algo = normalizeDigestName(algo); this._chunks = []; this._closed = false; }
  update(data, encoding) {
    if (this._closed) throw new Error("Digest already finalized");
    this._chunks.push(asBytes(data, encoding));
    return this;
  }
  digest(encoding) {
    if (this._closed) throw new Error("Digest already finalized");
    this._closed = true;
    const out = Buffer.from(ops.op_crypto_hash(this._algo, concat(this._chunks)));
    return encoding ? out.toString(encoding) : out;
  }
  copy() {
    const c = new Hash(this._algo);
    c._chunks = this._chunks.slice();
    return c;
  }
}

class Hmac {
  constructor(algo, key) {
    this._algo = normalizeDigestName(algo);
    this._key = asBytes(key);
    this._chunks = [];
    this._closed = false;
  }
  update(data, encoding) {
    if (this._closed) throw new Error("HMAC already finalized");
    this._chunks.push(asBytes(data, encoding));
    return this;
  }
  digest(encoding) {
    if (this._closed) throw new Error("HMAC already finalized");
    this._closed = true;
    const out = Buffer.from(ops.op_crypto_hmac(this._algo, this._key, concat(this._chunks)));
    return encoding ? out.toString(encoding) : out;
  }
}

function createHash(algo) { return new Hash(algo); }
function createHmac(algo, key) { return new Hmac(algo, key); }

function randomBytes(size, cb) {
  const buf = Buffer.from(ops.op_crypto_random_bytes(size));
  if (cb) { queueMicrotask(() => cb(null, buf)); return undefined; }
  return buf;
}
function randomFillSync(buf, offset = 0, size) {
  const len = size === undefined ? buf.length - offset : size;
  const random = ops.op_crypto_random_bytes(len);
  if (buf instanceof Uint8Array) buf.set(random, offset);
  else throw new TypeError("randomFillSync requires a Uint8Array");
  return buf;
}
function randomFill(buf, offset, size, cb) {
  if (typeof offset === "function") { cb = offset; offset = 0; size = buf.length; }
  else if (typeof size === "function") { cb = size; size = buf.length - offset; }
  try {
    randomFillSync(buf, offset, size);
    queueMicrotask(() => cb(null, buf));
  } catch (err) {
    queueMicrotask(() => cb(err));
  }
}
function randomUUID() { return ops.op_crypto_random_uuid(); }
function randomInt(min, max, cb) {
  if (max === undefined) { max = min; min = 0; }
  if (typeof max === "function") { cb = max; max = min; min = 0; }
  if (!Number.isInteger(min) || !Number.isInteger(max) || max <= min) {
    const err = new RangeError("randomInt: max must be > min and both must be integers");
    if (cb) return queueMicrotask(() => cb(err));
    throw err;
  }
  const range = max - min;
  const bytes = ops.op_crypto_random_bytes(6);
  let v = 0;
  for (let i = 0; i < 6; i++) v = v * 256 + bytes[i];
  const result = min + (v % range);
  if (cb) { queueMicrotask(() => cb(null, result)); return undefined; }
  return result;
}

function timingSafeEqual(a, b) {
  return ops.op_crypto_timing_safe_equal(asBytes(a), asBytes(b));
}

function pbkdf2Sync(password, salt, iterations, keylen, digest) {
  return Buffer.from(
    ops.op_crypto_pbkdf2(asBytes(password), asBytes(salt), iterations, keylen, normalizeDigestName(digest)),
  );
}
function pbkdf2(password, salt, iterations, keylen, digest, cb) {
  queueMicrotask(() => {
    try { cb(null, pbkdf2Sync(password, salt, iterations, keylen, digest)); }
    catch (err) { cb(err); }
  });
}

function scryptSync(password, salt, keylen, options = {}) {
  const N = options.N ?? options.cost ?? 16384;
  const r = options.r ?? options.blockSize ?? 8;
  const p = options.p ?? options.parallelization ?? 1;
  return Buffer.from(
    ops.op_crypto_scrypt(asBytes(password), asBytes(salt), keylen, N, r, p),
  );
}
function scrypt(password, salt, keylen, options, cb) {
  if (typeof options === "function") { cb = options; options = {}; }
  queueMicrotask(() => {
    try { cb(null, scryptSync(password, salt, keylen, options)); }
    catch (err) { cb(err); }
  });
}

const AEAD_GCM = "aes-256-gcm";
const AEAD_CHACHA = "chacha20-poly1305";
const NON_AEAD_AES = new Set([
  "aes-128-cbc", "aes-192-cbc", "aes-256-cbc",
  "aes-128-ctr", "aes-256-ctr",
]);

class CipherIv {
  constructor(algo, key, iv, mode) {
    this._algo = String(algo).toLowerCase();
    this._mode = mode;
    this._key = asBytes(key);
    this._iv = asBytes(iv);
    this._aad = new Uint8Array(0);
    this._chunks = [];
    this._tagLength = 16;
    this._authTag = null;
    this._finalised = false;
    if (this._algo !== AEAD_GCM && this._algo !== AEAD_CHACHA && !NON_AEAD_AES.has(this._algo)) {
      const err = new Error(`Unsupported cipher "${algo}" in nexide`);
      err.code = "ERR_CRYPTO_UNKNOWN_CIPHER";
      throw err;
    }
  }
  setAAD(buf) { this._aad = asBytes(buf); return this; }
  setAuthTag(buf) {
    if (this._mode !== "decrypt") throw new Error("setAuthTag valid for decrypt only");
    this._authTag = asBytes(buf);
    return this;
  }
  setAutoPadding(_flag) { return this; }
  update(data, inputEncoding, outputEncoding) {
    this._chunks.push(asBytes(data, inputEncoding));
    if (outputEncoding) return Buffer.alloc(0).toString(outputEncoding);
    return Buffer.alloc(0);
  }
  final(encoding) {
    if (this._finalised) throw new Error("Cipher already finalized");
    this._finalised = true;
    const input = concat(this._chunks);
    let out;
    if (this._algo === AEAD_GCM) {
      if (this._mode === "encrypt") {
        const sealed = Buffer.from(ops.op_crypto_aes_gcm_seal(this._key, this._iv, input, this._aad));
        this._authTag = sealed.subarray(sealed.length - this._tagLength);
        out = sealed.subarray(0, sealed.length - this._tagLength);
      } else {
        if (!this._authTag) throw new Error("authTag required for decrypt");
        const ct = Buffer.concat([Buffer.from(input), Buffer.from(this._authTag)]);
        out = Buffer.from(ops.op_crypto_aes_gcm_open(this._key, this._iv, ct, this._aad));
      }
    } else if (this._algo === AEAD_CHACHA) {
      if (this._mode === "encrypt") {
        const sealed = Buffer.from(ops.op_crypto_chacha20_seal(this._key, this._iv, input, this._aad));
        this._authTag = sealed.subarray(sealed.length - this._tagLength);
        out = sealed.subarray(0, sealed.length - this._tagLength);
      } else {
        if (!this._authTag) throw new Error("authTag required for decrypt");
        const ct = Buffer.concat([Buffer.from(input), Buffer.from(this._authTag)]);
        out = Buffer.from(ops.op_crypto_chacha20_open(this._key, this._iv, ct, this._aad));
      }
    } else if (this._mode === "encrypt") {
      out = Buffer.from(ops.op_crypto_aes_encrypt(this._algo, this._key, this._iv, input));
    } else {
      out = Buffer.from(ops.op_crypto_aes_decrypt(this._algo, this._key, this._iv, input));
    }
    return encoding ? out.toString(encoding) : out;
  }
  getAuthTag() {
    if (!this._authTag) throw new Error("auth tag unavailable before final()");
    return Buffer.from(this._authTag);
  }
}

function createCipheriv(algo, key, iv) { return new CipherIv(algo, key, iv, "encrypt"); }
function createDecipheriv(algo, key, iv) { return new CipherIv(algo, key, iv, "decrypt"); }

const SIGN_ALGO_MAP = {
  "rsa-sha256": "rsa-sha256", "sha256withrsaencryption": "rsa-sha256",
  "rsa-sha384": "rsa-sha384", "sha384withrsaencryption": "rsa-sha384",
  "rsa-sha512": "rsa-sha512", "sha512withrsaencryption": "rsa-sha512",
  "ecdsa-with-sha256": "ecdsa-p256-sha256",
  "ed25519": "ed25519",
};

const ED25519_OID_BYTES = new Uint8Array([0x06, 0x03, 0x2b, 0x65, 0x70]);
const EC_OID_BYTES = new Uint8Array([0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
const RSA_OID_BYTES = new Uint8Array([
  0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
]);

function indexOfBytes(haystack, needle) {
  outer: for (let i = 0; i + needle.length <= haystack.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer;
    }
    return i;
  }
  return -1;
}

function detectKeyAlgo(pem) {
  if (/BEGIN RSA (PUBLIC|PRIVATE) KEY/.test(pem)) return "rsa";
  if (/BEGIN EC (PUBLIC|PRIVATE) KEY/.test(pem)) return "ecdsa-p256";
  const match = pem.match(/-----BEGIN[^-]+-----([\s\S]*?)-----END[^-]+-----/);
  if (!match) return null;
  let body;
  try { body = Buffer.from(match[1].replace(/\s+/g, ""), "base64"); }
  catch (_err) { return null; }
  if (indexOfBytes(body, ED25519_OID_BYTES) >= 0) return "ed25519";
  if (indexOfBytes(body, EC_OID_BYTES) >= 0) return "ecdsa-p256";
  if (indexOfBytes(body, RSA_OID_BYTES) >= 0) return "rsa";
  return null;
}

function resolveSignAlgo(algo, key) {
  const lookup = String(algo).toLowerCase();
  if (SIGN_ALGO_MAP[lookup]) return SIGN_ALGO_MAP[lookup];
  if (typeof key === "object" && key && key.type === "ed25519") return "ed25519";
  const pem = (() => {
    try { return pemFromKeyLike(key); } catch (_err) { return null; }
  })();
  const keyType = pem ? detectKeyAlgo(pem) : null;
  if (lookup === "sha256" || lookup === "rsa-sha256") {
    if (keyType === "ecdsa-p256") return "ecdsa-p256-sha256";
    if (keyType === "ed25519") return "ed25519";
    return "rsa-sha256";
  }
  if (lookup === "sha384" || lookup === "rsa-sha384") return "rsa-sha384";
  if (lookup === "sha512" || lookup === "rsa-sha512") return "rsa-sha512";
  if (keyType === "ed25519") return "ed25519";
  return lookup;
}

function pemFromKeyLike(key) {
  if (typeof key === "string") return key;
  if (key && typeof key === "object") {
    if (typeof key.key === "string") return key.key;
    if (key.key instanceof Uint8Array) return new TextDecoder().decode(key.key);
  }
  if (key instanceof Uint8Array) return new TextDecoder().decode(key);
  throw new TypeError("Sign/Verify: key must be a PEM string, Buffer, or KeyObject");
}

class Sign {
  constructor(algo) { this._algo = algo; this._chunks = []; }
  update(data, encoding) { this._chunks.push(asBytes(data, encoding)); return this; }
  sign(key, encoding) {
    const algo = resolveSignAlgo(this._algo, key);
    const pem = pemFromKeyLike(key);
    const sig = Buffer.from(ops.op_crypto_sign(algo, pem, concat(this._chunks)));
    return encoding ? sig.toString(encoding) : sig;
  }
}

class Verify {
  constructor(algo) { this._algo = algo; this._chunks = []; }
  update(data, encoding) { this._chunks.push(asBytes(data, encoding)); return this; }
  verify(key, signature, signatureEncoding) {
    const algo = resolveSignAlgo(this._algo, key);
    const pem = pemFromKeyLike(key);
    const sigBytes = typeof signature === "string"
      ? Buffer.from(signature, signatureEncoding || "hex")
      : asBytes(signature);
    return ops.op_crypto_verify(algo, pem, concat(this._chunks), sigBytes);
  }
}

function createSign(algo) { return new Sign(algo); }
function createVerify(algo) { return new Verify(algo); }

const subtle = {
  async digest(algorithm, data) {
    const name = typeof algorithm === "string" ? algorithm : algorithm && algorithm.name;
    if (typeof name !== "string") throw new TypeError("digest: algorithm name required");
    const map = { "SHA-1": "sha1", "SHA-256": "sha256", "SHA-384": "sha384", "SHA-512": "sha512" };
    const algo = map[name.toUpperCase()] || name.toLowerCase().replace(/-/g, "");
    const bytes = data instanceof Uint8Array
      ? data
      : ArrayBuffer.isView(data)
        ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
        : data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : null;
    if (!bytes) throw new TypeError("digest: data must be BufferSource");
    const out = ops.op_crypto_hash(algo, bytes);
    return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
  },
};

const webcrypto = {
  randomUUID,
  getRandomValues(target) {
    if (!(target instanceof Uint8Array || ArrayBuffer.isView(target))) {
      throw new TypeError("getRandomValues requires a typed array");
    }
    const view = target instanceof Uint8Array
      ? target
      : new Uint8Array(target.buffer, target.byteOffset, target.byteLength);
    view.set(ops.op_crypto_random_bytes(view.length));
    return target;
  },
  subtle,
};

class CryptoKey {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

class SubtleCrypto {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

class Crypto {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

Object.setPrototypeOf(webcrypto, Crypto.prototype);
Object.setPrototypeOf(subtle, SubtleCrypto.prototype);

webcrypto.CryptoKey = CryptoKey;
webcrypto.SubtleCrypto = SubtleCrypto;
webcrypto.Crypto = Crypto;

const KEY_GATE = Symbol("nexide.crypto.KeyObjectGate");

function base64UrlEncode(buf) {
  return Buffer.from(buf).toString("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
}

function base64UrlDecode(s) {
  const pad = (4 - (s.length % 4)) % 4;
  return Buffer.from(s.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat(pad), "base64");
}

function curveToNamed(curve) {
  switch (curve) {
    case "P-256": case "p-256": case "prime256v1": case "secp256r1": return "prime256v1";
    case "P-384": case "p-384": case "secp384r1": return "secp384r1";
    case "P-521": case "p-521": case "secp521r1": return "secp521r1";
    default: return curve;
  }
}

function namedToJoseCurve(name) {
  switch (name) {
    case "prime256v1": case "secp256r1": return "P-256";
    case "secp384r1": return "P-384";
    case "secp521r1": return "P-521";
    default: return name;
  }
}

function pemLabelForKind(kind) {
  switch (kind) {
    case "private-pkcs8": return "PRIVATE KEY";
    case "public-spki": return "PUBLIC KEY";
    case "pkcs1-priv": return "RSA PRIVATE KEY";
    case "pkcs1-pub": return "RSA PUBLIC KEY";
    case "ec-sec1": return "EC PRIVATE KEY";
    default: throw new Error(`unknown kind: ${kind}`);
  }
}

function kindForPemLabel(label) {
  switch (label) {
    case "PRIVATE KEY": return "private-pkcs8";
    case "PUBLIC KEY": return "public-spki";
    case "RSA PRIVATE KEY": return "pkcs1-priv";
    case "RSA PUBLIC KEY": return "pkcs1-pub";
    case "EC PRIVATE KEY": return "ec-sec1";
    default: return null;
  }
}

class KeyObject {
  constructor(gate, fields) {
    if (gate !== KEY_GATE) {
      const err = new TypeError("Illegal constructor");
      err.code = "ERR_INVALID_THIS";
      throw err;
    }
    this._type = fields.type;
    this._kind = fields.kind || null;
    this._der = fields.der || null;
    this._info = fields.info || {};
    this._secret = fields.secret || null;
  }

  get type() { return this._type; }

  get asymmetricKeyType() {
    if (this._type === "secret") return undefined;
    return this._info.asymmetricKeyType;
  }

  get asymmetricKeyDetails() {
    if (this._type === "secret") return undefined;
    const details = {};
    if (this._info.modulusLength != null) details.modulusLength = this._info.modulusLength;
    if (this._info.publicExponent != null) details.publicExponent = BigInt(this._info.publicExponent);
    if (this._info.namedCurve != null) details.namedCurve = this._info.namedCurve;
    return details;
  }

  get symmetricKeySize() {
    return this._type === "secret" && this._secret ? this._secret.length : undefined;
  }

  export(options = {}) {
    if (this._type === "secret") {
      if (options.format === "jwk") {
        return { kty: "oct", k: base64UrlEncode(this._secret) };
      }
      return Buffer.from(this._secret);
    }
    const format = options.format || "pem";
    if (format === "jwk") {
      const json = ops.op_crypto_der_to_jwk(this._der, this._kind);
      return JSON.parse(json);
    }
    let outKind = this._kind;
    if (this._type === "private") {
      const t = options.type || "pkcs8";
      if (t === "pkcs8") outKind = "private-pkcs8";
      else if (t === "pkcs1") outKind = "pkcs1-priv";
      else if (t === "sec1") outKind = "ec-sec1";
      else throw new Error(`unsupported private export type: ${t}`);
    } else {
      const t = options.type || "spki";
      if (t === "spki") outKind = "public-spki";
      else if (t === "pkcs1") outKind = "pkcs1-pub";
      else throw new Error(`unsupported public export type: ${t}`);
    }
    let der = this._der;
    if (outKind !== this._kind) {
      const hint = this._info.namedCurve || "";
      der = ops.op_crypto_key_convert(this._kind, outKind, this._der, hint);
    }
    if (options.cipher || options.passphrase) {
      const err = new Error("Encrypted key export with passphrase is not supported in nexide");
      err.code = "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM";
      throw err;
    }
    if (format === "der") return Buffer.from(der);
    if (format === "pem") return ops.op_crypto_pem_encode(pemLabelForKind(outKind), der);
    throw new Error(`unsupported export format: ${format}`);
  }

  equals(other) {
    if (!(other instanceof KeyObject)) return false;
    if (this._type !== other._type) return false;
    if (this._type === "secret") {
      if (!this._secret || !other._secret) return false;
      if (this._secret.length !== other._secret.length) return false;
      let diff = 0;
      for (let i = 0; i < this._secret.length; i++) diff |= this._secret[i] ^ other._secret[i];
      return diff === 0;
    }
    if (this._kind !== other._kind || this._der.length !== other._der.length) return false;
    let diff = 0;
    for (let i = 0; i < this._der.length; i++) diff |= this._der[i] ^ other._der[i];
    return diff === 0;
  }

  static from(cryptoKey) {
    const err = new Error("KeyObject.from(CryptoKey) is not supported");
    err.code = "ERR_METHOD_NOT_IMPLEMENTED";
    throw err;
  }
}

function makeKeyObject(fields) { return new KeyObject(KEY_GATE, fields); }

function inspectAndBuild(type, kind, der) {
  const info = JSON.parse(ops.op_crypto_key_inspect(der, kind));
  return makeKeyObject({ type, kind, der, info });
}

function createSecretKey(key, encoding) {
  let buf;
  if (typeof key === "string") buf = Buffer.from(key, encoding || "utf8");
  else if (key instanceof Uint8Array) buf = Buffer.from(key);
  else if (key && typeof key === "object" && key.type === "Buffer" && Array.isArray(key.data)) buf = Buffer.from(key.data);
  else throw new TypeError("createSecretKey: key must be Buffer/Uint8Array/string");
  return makeKeyObject({ type: "secret", secret: new Uint8Array(buf) });
}

function jwkToKeyObject(jwk, wantPublic) {
  if (jwk.kty === "oct") {
    return createSecretKey(base64UrlDecode(jwk.k));
  }
  const wantKind = wantPublic ? "public-spki" : "private-pkcs8";
  const der = ops.op_crypto_jwk_to_der(JSON.stringify(jwk), wantKind);
  return inspectAndBuild(wantPublic ? "public" : "private", wantKind, der);
}

function normalizeKeyInput(input) {
  if (input instanceof KeyObject) return { kind: "keyobject", value: input };
  if (typeof input === "string") return { kind: "pem", value: input };
  if (input instanceof Uint8Array) return { kind: "der", value: input, type: "pkcs8" };
  if (input && typeof input === "object") {
    if (input.kty) return { kind: "jwk", value: input };
    if (input.format === "jwk" && input.key) return { kind: "jwk", value: input.key };
    if (typeof input.key === "string") return { kind: "pem", value: input.key };
    if (input.key instanceof Uint8Array || (input.key && input.key.type === "Buffer")) {
      const der = input.key instanceof Uint8Array ? input.key : Buffer.from(input.key.data);
      return { kind: "der", value: der, type: input.type || "pkcs8" };
    }
  }
  throw new TypeError("invalid key input");
}

function pemToDer(pem) {
  const decoded = ops.op_crypto_pem_decode(pem);
  return { label: decoded.label, der: decoded.der };
}

function createPrivateKey(input) {
  const norm = normalizeKeyInput(input);
  if (norm.kind === "keyobject") {
    if (norm.value._type !== "private") throw new Error("expected private KeyObject");
    return norm.value;
  }
  if (norm.kind === "jwk") return jwkToKeyObject(norm.value, false);
  let der, kind;
  if (norm.kind === "pem") {
    const { label, der: rawDer } = pemToDer(norm.value);
    kind = kindForPemLabel(label);
    if (!kind) throw new Error(`unsupported PEM label for private key: ${label}`);
    der = rawDer;
  } else {
    der = norm.value;
    if (norm.type === "pkcs1") kind = "pkcs1-priv";
    else if (norm.type === "sec1") kind = "ec-sec1";
    else kind = "private-pkcs8";
  }
  if (kind !== "private-pkcs8") {
    const hint = "";
    der = ops.op_crypto_key_convert(kind, "private-pkcs8", der, hint);
    kind = "private-pkcs8";
  }
  return inspectAndBuild("private", kind, der);
}

function createPublicKey(input) {
  if (input instanceof KeyObject && input._type === "private") {
    const der = ops.op_crypto_key_convert("private-pkcs8", "public-spki", input._der, "");
    return inspectAndBuild("public", "public-spki", der);
  }
  const norm = normalizeKeyInput(input);
  if (norm.kind === "keyobject") {
    if (norm.value._type === "public") return norm.value;
    if (norm.value._type === "private") {
      const der = ops.op_crypto_key_convert("private-pkcs8", "public-spki", norm.value._der, "");
      return inspectAndBuild("public", "public-spki", der);
    }
    throw new Error("cannot derive public key from secret KeyObject");
  }
  if (norm.kind === "jwk") return jwkToKeyObject(norm.value, true);
  let der, kind;
  if (norm.kind === "pem") {
    const { label, der: rawDer } = pemToDer(norm.value);
    kind = kindForPemLabel(label);
    if (!kind) throw new Error(`unsupported PEM label for public key: ${label}`);
    der = rawDer;
    if (kind === "private-pkcs8" || kind === "pkcs1-priv" || kind === "ec-sec1") {
      if (kind !== "private-pkcs8") der = ops.op_crypto_key_convert(kind, "private-pkcs8", der, "");
      der = ops.op_crypto_key_convert("private-pkcs8", "public-spki", der, "");
      kind = "public-spki";
    }
  } else {
    der = norm.value;
    if (norm.type === "pkcs1") kind = "pkcs1-pub";
    else kind = "public-spki";
  }
  if (kind === "pkcs1-pub") {
    der = ops.op_crypto_key_convert("pkcs1-pub", "public-spki", der, "");
    kind = "public-spki";
  }
  return inspectAndBuild("public", kind, der);
}

function generateKeyPairSync(type, options = {}) {
  const optsJson = JSON.stringify(options || {});
  const result = ops.op_crypto_generate_key_pair(type, optsJson);
  const info = JSON.parse(result.info_json);
  const pubKO = makeKeyObject({ type: "public", kind: "public-spki", der: result.publicKey, info });
  const privKO = makeKeyObject({ type: "private", kind: "private-pkcs8", der: result.privateKey, info });
  const pubEnc = options.publicKeyEncoding;
  const privEnc = options.privateKeyEncoding;
  return {
    publicKey: pubEnc ? pubKO.export(pubEnc) : pubKO,
    privateKey: privEnc ? privKO.export(privEnc) : privKO,
  };
}

function generateKeyPair(type, options, callback) {
  if (typeof options === "function") { callback = options; options = {}; }
  queueMicrotask(() => {
    try {
      const r = generateKeyPairSync(type, options);
      callback(null, r.publicKey, r.privateKey);
    } catch (err) {
      callback(err);
    }
  });
}

function generateKeySync(type, options = {}) {
  let length = options.length;
  if (type === "hmac") {
    if (length == null) length = 256;
    if (length < 8 || length > 65536 || length % 8 !== 0) throw new RangeError("hmac length out of range");
  } else if (type === "aes") {
    if (length !== 128 && length !== 192 && length !== 256) throw new RangeError("aes length must be 128/192/256");
  } else {
    throw new Error(`generateKey: unsupported type ${type}`);
  }
  const bytes = Buffer.from(ops.op_crypto_random_bytes(length / 8));
  return createSecretKey(bytes);
}

function generateKey(type, options, callback) {
  if (typeof options === "function") { callback = options; options = {}; }
  queueMicrotask(() => {
    try { callback(null, generateKeySync(type, options)); } catch (err) { callback(err); }
  });
}

function rsaPaddingName(p) {
  if (p == null || p === 4) return "oaep";
  if (p === 1) return "pkcs1";
  throw new Error(`unsupported RSA padding: ${p}`);
}

function rsaCryptOptions(input, defaultPadding) {
  let key, padding, oaepHash, oaepLabel;
  if (input instanceof KeyObject || typeof input === "string" || input instanceof Uint8Array) {
    key = input;
    padding = defaultPadding;
    oaepHash = "sha1";
    oaepLabel = null;
  } else if (input && typeof input === "object") {
    if (input.kty) {
      key = input;
    } else {
      key = input.key !== undefined ? input.key : input;
    }
    padding = input.padding != null ? input.padding : defaultPadding;
    oaepHash = (input.oaepHash || "sha1").toLowerCase();
    oaepLabel = input.oaepLabel ? asBytes(input.oaepLabel) : null;
  } else {
    throw new TypeError("invalid RSA key argument");
  }
  return { key, padding: rsaPaddingName(padding), oaepHash, oaepLabel };
}

function publicEncrypt(input, buffer) {
  const { key, padding, oaepHash, oaepLabel } = rsaCryptOptions(input, 4);
  const ko = createPublicKey(key);
  const data = asBytes(buffer);
  const out = ops.op_crypto_rsa_encrypt(ko._der, data, padding, oaepHash, oaepLabel || new Uint8Array(0));
  return Buffer.from(out);
}

function privateDecrypt(input, buffer) {
  const { key, padding, oaepHash, oaepLabel } = rsaCryptOptions(input, 4);
  const ko = createPrivateKey(key);
  const data = asBytes(buffer);
  const out = ops.op_crypto_rsa_decrypt(ko._der, data, padding, oaepHash, oaepLabel || new Uint8Array(0));
  return Buffer.from(out);
}

function privateEncrypt(_input, _buffer) {
  const err = new Error("privateEncrypt is not supported in nexide");
  err.code = "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM";
  throw err;
}

function publicDecrypt(_input, _buffer) {
  const err = new Error("publicDecrypt is not supported in nexide");
  err.code = "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM";
  throw err;
}

function digestForAlgo(algorithm) {
  if (!algorithm) return null;
  return normalizeDigestName(algorithm);
}

function chooseSignAlgo(keyObject, algorithm, padding) {
  const akt = keyObject.asymmetricKeyType;
  const digest = digestForAlgo(algorithm);
  if (akt === "ed25519") return { algo: "ed25519" };
  if (akt === "rsa" || akt === "rsa-pss") {
    if (!digest) throw new Error("RSA sign requires a digest algorithm");
    const isPss = padding === 6 || akt === "rsa-pss";
    return { algo: `${isPss ? "rsa-pss-" : "rsa-"}${digest}` };
  }
  if (akt === "ec") {
    const curve = curveToNamed(keyObject._info.namedCurve);
    if (curve === "prime256v1") return { algo: "ecdsa-p256-sha256" };
    if (curve === "secp384r1") return { algo: "ecdsa-p384-sha384" };
    if (curve === "secp521r1") return { algo: "ecdsa-p521-sha512" };
    throw new Error(`unsupported EC curve: ${curve}`);
  }
  throw new Error(`unsupported asymmetricKeyType: ${akt}`);
}

function signOneShot(algorithm, data, key) {
  const ko = key instanceof KeyObject ? key : createPrivateKey(key);
  const padding = (key && typeof key === "object" && !(key instanceof KeyObject)) ? key.padding : null;
  const { algo } = chooseSignAlgo(ko, algorithm, padding);
  const dsaEncoding = (key && typeof key === "object" && !(key instanceof KeyObject) && key.dsaEncoding) || "der";
  const format = algo.startsWith("ecdsa-") ? dsaEncoding : "der";
  const sig = ops.op_crypto_sign_der(algo, "private-pkcs8", ko._der, asBytes(data), format);
  return Buffer.from(sig);
}

function verifyOneShot(algorithm, data, key, signature) {
  const ko = key instanceof KeyObject ? key : createPublicKey(key);
  const padding = (key && typeof key === "object" && !(key instanceof KeyObject)) ? key.padding : null;
  const { algo } = chooseSignAlgo(ko, algorithm, padding);
  const dsaEncoding = (key && typeof key === "object" && !(key instanceof KeyObject) && key.dsaEncoding) || "auto";
  const format = algo.startsWith("ecdsa-") ? dsaEncoding : "der";
  return ops.op_crypto_verify_der(algo, "public-spki", ko._der, asBytes(data), asBytes(signature), format);
}

function diffieHellman(opts) {
  if (!opts || !(opts.privateKey instanceof KeyObject) || !(opts.publicKey instanceof KeyObject)) {
    throw new TypeError("diffieHellman requires {privateKey, publicKey} KeyObjects");
  }
  const priv = opts.privateKey;
  const pub = opts.publicKey;
  const akt = priv.asymmetricKeyType;
  if (akt !== pub.asymmetricKeyType) {
    const err = new Error("diffieHellman: incompatible key types");
    err.code = "ERR_CRYPTO_INCOMPATIBLE_KEY";
    throw err;
  }
  if (akt === "x25519") {
    return Buffer.from(ops.op_crypto_x25519_derive(priv._der, pub._der));
  }
  if (akt === "ec") {
    const curveJose = namedToJoseCurve(priv._info.namedCurve);
    return Buffer.from(ops.op_crypto_ecdh_derive(curveJose, priv._der, pub._der));
  }
  throw new Error(`diffieHellman not supported for type: ${akt}`);
}

class ECDH {
  constructor(curve) {
    const named = curveToNamed(curve);
    if (!["prime256v1", "secp384r1", "secp521r1"].includes(named)) {
      throw new Error(`unsupported ECDH curve: ${curve}`);
    }
    this._curve = named;
    this._joseCurve = namedToJoseCurve(named);
    this._privDer = null;
    this._pubDer = null;
    this._privRaw = null;
    this._pubRaw = null;
  }
  generateKeys(encoding, format) {
    const r = ops.op_crypto_ecdh_generate(this._joseCurve);
    this._privDer = r.privateKey;
    this._pubDer = r.publicKey;
    this._privRaw = r.privateRaw;
    this._pubRaw = r.publicRaw;
    return this.getPublicKey(encoding, format);
  }
  getPublicKey(encoding, _format) {
    if (!this._pubRaw) throw new Error("ECDH: keys not generated");
    const buf = Buffer.from(this._pubRaw);
    return encoding ? buf.toString(encoding) : buf;
  }
  getPrivateKey(encoding) {
    if (!this._privRaw) throw new Error("ECDH: keys not generated");
    const buf = Buffer.from(this._privRaw);
    return encoding ? buf.toString(encoding) : buf;
  }
  setPrivateKey(priv, encoding) {
    const raw = typeof priv === "string" ? Buffer.from(priv, encoding || "hex") : asBytes(priv);
    const r = ops.op_crypto_ecdh_from_raw(this._joseCurve, raw);
    this._privDer = r.privateKey;
    this._pubDer = r.publicKey;
    this._privRaw = r.privateRaw;
    this._pubRaw = r.publicRaw;
    return this;
  }
  computeSecret(otherPub, inEnc, outEnc) {
    if (!this._privRaw) throw new Error("ECDH: keys not generated");
    let pubRaw;
    if (typeof otherPub === "string") pubRaw = Buffer.from(otherPub, inEnc || "hex");
    else pubRaw = asBytes(otherPub);
    const secret = Buffer.from(ops.op_crypto_ecdh_compute_raw(this._joseCurve, this._privRaw, pubRaw));
    return outEnc ? secret.toString(outEnc) : secret;
  }
}

function createECDH(curve) { return new ECDH(curve); }

function createDiffieHellman(_a, _b, _c, _d) {
  const err = new Error("createDiffieHellman is not supported in nexide");
  err.code = "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM";
  throw err;
}

function createDiffieHellmanGroup(_name) {
  const err = new Error("createDiffieHellmanGroup is not supported in nexide");
  err.code = "ERR_FEATURE_UNAVAILABLE_ON_PLATFORM";
  throw err;
}

function hkdfSync(digest, ikm, salt, info, keylen) {
  const ikmBuf = ikm instanceof KeyObject && ikm._type === "secret" ? ikm._secret : asBytes(ikm);
  const out = ops.op_crypto_hkdf(
    normalizeDigestName(digest),
    ikmBuf,
    asBytes(salt || new Uint8Array(0)),
    asBytes(info || new Uint8Array(0)),
    keylen,
  );
  return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
}

function hkdf(digest, ikm, salt, info, keylen, callback) {
  queueMicrotask(() => {
    try { callback(null, hkdfSync(digest, ikm, salt, info, keylen)); } catch (err) { callback(err); }
  });
}

module.exports = {
  createHash,
  createHmac,
  randomBytes,
  randomFill,
  randomFillSync,
  randomUUID,
  randomInt,
  timingSafeEqual,
  pbkdf2,
  pbkdf2Sync,
  scrypt,
  scryptSync,
  createCipheriv,
  createDecipheriv,
  createSign,
  createVerify,
  Sign,
  Verify,
  Hash,
  Hmac,
  webcrypto,
  Crypto,
  CryptoKey,
  SubtleCrypto,
  KeyObject,
  createPrivateKey,
  createPublicKey,
  createSecretKey,
  generateKeyPair,
  generateKeyPairSync,
  generateKey,
  generateKeySync,
  diffieHellman,
  createDiffieHellman,
  createDiffieHellmanGroup,
  createECDH,
  ECDH,
  privateDecrypt,
  publicEncrypt,
  privateEncrypt,
  publicDecrypt,
  sign: signOneShot,
  verify: verifyOneShot,
  hkdf,
  hkdfSync,
  getCipherInfo: () => null,
  getCurves: () => ["prime256v1", "secp384r1", "secp521r1"],
  getFips: () => 0,
  setFips: () => {},
  constants: {
    RSA_PKCS1_PADDING: 1,
    RSA_NO_PADDING: 3,
    RSA_PKCS1_OAEP_PADDING: 4,
    RSA_PKCS1_PSS_PADDING: 6,
    RSA_PSS_SALTLEN_DIGEST: -1,
    RSA_PSS_SALTLEN_MAX_SIGN: -2,
    RSA_PSS_SALTLEN_AUTO: -2,
  },
  getCiphers: () => [
    "aes-128-cbc", "aes-192-cbc", "aes-256-cbc",
    "aes-128-ctr", "aes-256-ctr",
    "aes-256-gcm", "chacha20-poly1305",
  ],
  getHashes: () => ["sha1", "sha256", "sha384", "sha512", "md5"],
};
