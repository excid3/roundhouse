# Security

Roundhouse is in early-stage, fast moving and has not been audited for security.
Treat the apps it compiles accordingly: they are not hardened, and their runtime
may have vulnerabilities. This does not mean that we are not taking security
seriously, community members actively file security fixes and we welcome reports
from everyone.

## How to report a problem

- If you have Hardening ideas, found a weakness in tooling or low level
  vulnerability, please open a public issue or pull request as usual.
- If you found a high level security issue that could be used to exploit a
  compiled app and cause serious damage, please report privately through
  [GitHub's private vulnerability
  reporting](https://github.com/rubys/roundhouse/security/advisories/new).
  Please include reproduction steps and the targets you checked and if you
  include a patch too, we will be forever grateful.

If you are unsure which applies, report it privately. Also, if you use AI for
discovery, please make sure the findings are actually legitimate before
reporting.

## Fixes

Fixes land as ordinary pull requests with `[Security]` in the title,
and credit the reporter unless they ask otherwise.
