# i18n e2e fixture

Minimal Next.js 16 app exercising real-world i18n libraries on top of
nexide:

- `i18next` + `i18next-resources-to-backend` — JSON resource loading
  via dynamic `import()` from server components
- `next-i18n-router` — locale-prefixed routes (`/en`, `/pl`) with
  automatic redirect from `/`
- `Intl.*` — `NumberFormat`, `DateTimeFormat`, `Collator`, no-arg
  `toLocaleString()`
- Polish UTF-8 multi-byte text in both translation JSON and inline
  source

Routes:

| Path                | Mode      | Notes                                             |
| ------------------- | --------- | ------------------------------------------------- |
| `/`                 | redirect  | `next-i18n-router` middleware → `/en` or `/pl`    |
| `/[lang]`           | dynamic   | Translated index using `t()` server-side          |
| `/[lang]/utf8`      | dynamic   | Polish diacritics stress test (RSC Flight stream) |
| `/[lang]/intl`      | dynamic   | Explicit + default locale `Intl.*` calls          |
| `/[lang]/static`    | static    | Prerendered HTML (`generateStaticParams`)         |
| `/api/ping`         | route     | Liveness probe                                    |
| `/api/format`       | route     | JSON snapshot of all `Intl.*` results             |

## Build & run

```bash
cd e2e/i18n
pnpm install
pnpm build
cd ../..
cargo build --release
cargo test -p nexide-e2e --release i18n -- --ignored --nocapture
```

The integration test runs the runtime in an explicitly empty locale
environment (`LANG=` / `LC_ALL=`) to mirror production failures seen
on slim images.

