# Deploying, and installing into a cluster somebody else owns

The rules for `deploy/` — the Dockerfiles, the compose stack, `helm/aiwatcher`,
`helmfile.yaml.gotmpl` and `scripts/detect-stack.py`. Read
[ADR_0009](../docs/ADR/ADR_0009_INSTALL_BY_DETECTION.md) and
[ADR_0006](../docs/ADR/ADR_0006_LOCAL_K8S_WITH_TILT.md); `docs/INSTALL.md` is
the planner case. Every backend is `install | external | none`, never a boolean.

- **Never attach a NetworkPolicy to pods this chart does not own unless they are
  already fenced.** Policies are additive, so a rule added to pods some policy
  already restricts widens them by one path — which is how the Collector reaches
  planner's VictoriaMetrics without editing planner's chart. Added to pods *no*
  policy selects, the same rule narrows them from "accepts everything" to
  "accepts aiwatcher only" and cuts off whoever was already talking to them.
  `detect-stack.py` reports `fenced`; only that turns the rule on. The object
  store is the sharp case: planner's RustFS serves three of its own workloads,
  and only `fenced` turns `allowEgressToExternalPromptStore` on.
- **Never let installation reuse a Collector it merely found.** A foreign
  Collector almost certainly lacks the `attributes/redact` processor, which is
  the whole reason the Collector is in the path. Detection reports one;
  `collector.mode: external` stays a human decision.
- **Never reuse a database the cluster happens to be running.**
  `detect-stack.py` reports PostgreSQL and derives nothing: what this release
  would do with a database it found is *create tables in it*, and which
  database, whose credentials and whether that role may create a schema are none
  of them discoverable from a matching image.
- **Never point Tilt at a non-local cluster.** The guard is in two places
  (`Tiltfile` and `just _assert-local-context`); do not weaken either. A typo in
  a context name must not be the only thing between a keystroke and production.
