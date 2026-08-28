# aiwatcher on a local test Kubernetes.
#
#   just tilt-up      the whole stack, rebuilt on save
#   just tilt-down    tear it down
#
# What this gets you that `just stack-up` (docker compose) does not: the wiring
# exercised the way it runs in a cluster — Services, probes, a ConfigMap the
# Collector actually reads, and Iggy behind a StatefulSet — plus a rebuild loop
# that reflects a Rust change in seconds rather than a full image build.

# ─────────────────────────────────────────────────────────────────────────────
# Safety
#
# This kubeconfig contains production EKS contexts. Tilt applies manifests to
# whatever context is current, so the guard is a hard stop rather than a prompt
# and the allowlist is explicit rather than inherited from Tilt's defaults.
# ─────────────────────────────────────────────────────────────────────────────

LOCAL_CONTEXTS = [
    'orbstack',
    'docker-desktop',
    'minikube',
    'colima',
    'rancher-desktop',
]
LOCAL_PREFIXES = ['kind-', 'k3d-', 'k3s-']

def is_local(context):
    if context in LOCAL_CONTEXTS:
        return True
    for prefix in LOCAL_PREFIXES:
        if context.startswith(prefix):
            return True
    return False

context = k8s_context()
if not is_local(context):
    fail(
        'refusing to run against "%s": it is not a known-local cluster.\n' % context +
        '  This kubeconfig has production clusters in it, and Tilt would apply to\n' +
        '  whichever context is current.\n' +
        '  Switch with: kubectl config use-context orbstack\n' +
        '  or run:      just tilt-up  (which checks the same thing first)'
    )
allow_k8s_contexts(context)
print('aiwatcher → local cluster "%s"' % context)

# ─────────────────────────────────────────────────────────────────────────────
# The stack
# ─────────────────────────────────────────────────────────────────────────────

# `AIWATCHER_K8S_OVERLAY=laser tilt up` swaps the write-ahead log for the Laser
# backend. The base is the default so this stack comes up green on any machine.
overlay = os.getenv('AIWATCHER_K8S_OVERLAY', 'base')
if overlay not in ['base', 'laser']:
    fail('AIWATCHER_K8S_OVERLAY must be "base" or "laser", got "%s"' % overlay)
print('aiwatcher → %s backend' % ('laser' if overlay == 'laser' else 'write-ahead log'))

k8s_yaml(kustomize('deploy/k8s/' + overlay))

# The image is built in a container, not on the host.
#
# A host build would be faster — it reuses the local target/ cache — but on
# macOS it produces a Mach-O binary, and the pod then dies with "Cannot run
# macOS (Mach-O) executable in Docker: Exec format error". Cross-compiling to
# linux/aarch64 would need `cross` or `cargo-zigbuild` installed, which is a
# setup step this should not require.
#
# The Dockerfile copies manifests before sources, so a source-only change
# reuses the dependency layer and only recompiles the six workspace crates.
build_features = 'aiwatcher-server/laser' if overlay == 'laser' else ''

docker_build(
    'aiwatcher',
    context = '.',
    dockerfile = 'deploy/Dockerfile',
    build_args = {'FEATURES': build_features},
    # Everything the Dockerfile reads, and nothing else: the panel, docs and
    # target/ would otherwise be shipped to the daemon on every build.
    only = ['crates', 'Cargo.toml', 'rust-toolchain.toml'],
)

# On the base backend aiwatcher does not need Iggy at all, so waiting for it
# would stall a stack that is otherwise ready.
aiwatcher_deps = ['otel-collector']
if overlay == 'laser':
    aiwatcher_deps.append('iggy')

k8s_resource(
    'aiwatcher',
    port_forwards = ['8080:8080'],
    resource_deps = aiwatcher_deps,
    labels = ['aiwatcher'],
)

k8s_resource('iggy', port_forwards = ['8090:8090'], labels = ['log'])
k8s_resource('victoriatraces', port_forwards = ['10428:10428'], labels = ['storage'])
k8s_resource('victoriametrics', port_forwards = ['8428:8428'], labels = ['storage'])
k8s_resource(
    'otel-collector',
    resource_deps = ['victoriatraces', 'victoriametrics'],
    labels = ['storage'],
)
k8s_resource(
    'grafana',
    port_forwards = ['3000:3000'],
    resource_deps = ['victoriatraces', 'victoriametrics'],
    labels = ['ui'],
)

# ─────────────────────────────────────────────────────────────────────────────
# The panel
#
# Served by Vite on the host rather than as a pod: a browser reload beats an
# image rebuild, and the dev server proxies /api to the port-forward above, so
# SSE and WebSocket behave exactly as they do behind one ingress.
# ─────────────────────────────────────────────────────────────────────────────

local_resource(
    'panel',
    serve_cmd = 'npm run dev',
    serve_dir = 'apps/panel',
    links = [link('http://localhost:5173', 'panel')],
    resource_deps = ['aiwatcher'],
    labels = ['ui'],
    allow_parallel = True,
)

# ─────────────────────────────────────────────────────────────────────────────
# On-demand
# ─────────────────────────────────────────────────────────────────────────────

# Publish a demo run. Triggered by hand from the Tilt UI.
local_resource(
    'seed',
    cmd = 'AIWATCHER_URL=http://localhost:8080 ./scripts/seed-demo-run.sh',
    resource_deps = ['aiwatcher'],
    trigger_mode = TRIGGER_MODE_MANUAL,
    auto_init = False,
    labels = ['tools'],
)

# The Laser adapter against the Iggy running in the cluster, through the
# port-forward. This is the suite that cannot run in plain `just test`.
local_resource(
    'test-laser',
    cmd = 'AIWATCHER_LASER_CONNECTION_STRING=iggy:iggy@127.0.0.1:8090 ' +
          'cargo test -p aiwatcher-bus --features laser --test laser_integration ' +
          '-- --ignored --test-threads=1',
    resource_deps = ['iggy'],
    trigger_mode = TRIGGER_MODE_MANUAL,
    auto_init = False,
    labels = ['tools'],
)
