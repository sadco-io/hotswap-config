# Security Policy

## Supported Versions

| Version | Supported | Notes |
|---------|-----------|-------|
| Latest published minor release | Yes | Before 1.0, only the latest minor release line receives security fixes. |
| Earlier releases | No | Upgrade to the latest published minor release. |

## Scope

`hotswap-config` loads, merges, validates, and atomically swaps configuration across `FileSource`, `EnvSource`, and the optional `HttpSource` (`remote` and `remote-native-tls`), and supports runtime updates via `PartialUpdate` (`partial-updates`), `Rollback` (`rollback`), and `GradualRollout` (`gradual-rollout`). Validation requires a registered validator.

- **In scope:** Vulnerabilities in config loading, deep-merging, deserialization, or RFC 6902 JSON Patch application reachable from untrusted or partially trusted config payloads; bypasses of registered validation (`with_validation` / `Validate`); torn reads or non-atomic state transitions during reload, partial update, rollback, or gradual rollout; panics or unbounded resource consumption on malformed inputs; and leakage of `HttpSource` secrets (bearer tokens or Basic-auth passwords set via `with_auth_token` / `with_basic_auth`, or passwords in URL userinfo) in `Debug` output or errors.
- **Out of scope:** Security properties not implemented by the crate—specifically, `HttpSource` does not support certificate pinning or payload signature verification—as well as caller-supplied plaintext `http://` URLs, the documented `PartialUpdate` caveat around `#[serde(skip_serializing)]`, and vulnerabilities in dependencies such as `config`, `json-patch`, `reqwest`, or `notify` (which belong to the upstream dependency, though we will release an update when needed).

## Advisories

For each security fix we publish a GitHub Security Advisory for `hotswap-config` and request a RustSec
advisory. Fixed versions are listed in the advisory and in [CHANGELOG.md](CHANGELOG.md).

## Reporting a Vulnerability

If you have discovered a security vulnerability in this project, please report
it privately. **Do not disclose it as a public issue.** This gives us time to
work with you to fix the issue before public exposure, reducing the chance that
the exploit will be used before a patch is released.

Please disclose it at [security advisory](https://github.com/sadco-io/hotswap-config/security/advisories/new).

This project is maintained by a team of volunteers on a reasonable-effort basis.
As such, please give us at least 90 days to work on a fix before public exposure.
