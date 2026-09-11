{{/*
Names and labels.

`fullnameOverride` defaults to "aiwatcher" rather than to the release name on
purpose: this chart is designed to be installed *into a namespace that already
belongs to something else* (see values-planner.yaml), where a resource called
`planner-aiwatcher-server` reads as part of planner and a resource called
`aiwatcher-server` reads as a guest. Set it if two aiwatcher releases ever have
to share one namespace.
*/}}
{{- define "aiwatcher.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aiwatcher.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- include "aiwatcher.name" . | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "aiwatcher.labels" -}}
app.kubernetes.io/name: {{ include "aiwatcher.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: aiwatcher
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
{{- end -}}

{{- define "aiwatcher.selectorLabels" -}}
app.kubernetes.io/name: {{ include "aiwatcher.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "aiwatcher.storageClass" -}}
{{- with .storageClassName }}
storageClassName: {{ . | quote }}
{{- end }}
{{- end -}}

{{/*
Where traces go.

Three modes, and the difference matters: `install` puts VictoriaTraces in this
release, `external` points at one that already exists, `none` means this
deployment keeps no trace history — the panel still shows live runs, because it
reads the read model rather than the trace store (ADR_0003).
*/}}
{{- define "aiwatcher.tracesBaseUrl" -}}
{{- $vt := .Values.victoriatraces -}}
{{- if eq $vt.mode "install" -}}
http://{{ include "aiwatcher.fullname" . }}-victoriatraces:10428
{{- else if eq $vt.mode "external" -}}
{{- if not $vt.external.url -}}
{{- fail "victoriatraces.mode is \"external\" but victoriatraces.external.url is empty. Set the URL, or set mode to \"install\" or \"none\"." -}}
{{- end -}}
{{- $vt.external.url | trimSuffix "/" -}}
{{- else if ne $vt.mode "none" -}}
{{- fail (printf "victoriatraces.mode is %q; it must be one of install, external, none." $vt.mode) -}}
{{- end -}}
{{- end -}}

{{- define "aiwatcher.metricsBaseUrl" -}}
{{- $vm := .Values.victoriametrics -}}
{{- if eq $vm.mode "install" -}}
http://{{ include "aiwatcher.fullname" . }}-victoriametrics:8428
{{- else if eq $vm.mode "external" -}}
{{- if not $vm.external.url -}}
{{- fail "victoriametrics.mode is \"external\" but victoriametrics.external.url is empty. Set the URL, or set mode to \"install\" or \"none\"." -}}
{{- end -}}
{{- $vm.external.url | trimSuffix "/" -}}
{{- else if ne $vm.mode "none" -}}
{{- fail (printf "victoriametrics.mode is %q; it must be one of install, external, none." $vm.mode) -}}
{{- end -}}
{{- end -}}

{{/*
What AIWATCHER_OTLP_ENDPOINT is set to.

The server appends `/v1/{signal}`, so this is a base URL, not a full path.
Empty means the server exports nothing and only serves the live view.
*/}}
{{- define "aiwatcher.otlpEndpoint" -}}
{{- $c := .Values.collector -}}
{{- if eq $c.mode "install" -}}
http://{{ include "aiwatcher.fullname" . }}-collector:4318
{{- else if eq $c.mode "external" -}}
{{- if not $c.external.endpoint -}}
{{- fail "collector.mode is \"external\" but collector.external.endpoint is empty." -}}
{{- end -}}
{{- $c.external.endpoint | trimSuffix "/" -}}
{{- else if eq $c.mode "none" -}}
{{- .Values.server.otlpEndpoint | trimSuffix "/" -}}
{{- else -}}
{{- fail (printf "collector.mode is %q; it must be one of install, external, none." $c.mode) -}}
{{- end -}}
{{- end -}}

{{/*
Where prompts are kept.

The same three modes as every backend, and the mode that matters most is
`none`: a deployment without a prompt store answers 501 on every
`/api/v1/prompts` route, which is a legitimate choice and is not the same thing
as an empty registry. The other two produce an S3 endpoint.

`install` points at this release's RustFS; `external` at any S3 — MinIO, Ceph,
AWS — because the adapter speaks S3 and nothing above it knows the difference.
*/}}
{{- define "aiwatcher.promptStoreEndpoint" -}}
{{- $ps := .Values.promptStore -}}
{{- if eq $ps.mode "install" -}}
http://{{ include "aiwatcher.fullname" . }}-rustfs:9000
{{- else if eq $ps.mode "external" -}}
{{- if not $ps.external.endpoint -}}
{{- fail "promptStore.mode is \"external\" but promptStore.external.endpoint is empty. Set the S3 endpoint, or set mode to \"install\" or \"none\"." -}}
{{- end -}}
{{- $ps.external.endpoint | trimSuffix "/" -}}
{{- else if ne $ps.mode "none" -}}
{{- fail (printf "promptStore.mode is %q; it must be one of install, external, none." $ps.mode) -}}
{{- end -}}
{{- end -}}

{{/*
The Secret holding the object store's credentials.

An `external` store must name one: this chart has no way to invent credentials
for a bucket somebody else owns, and defaulting to the release's own Secret
would produce a 403 at the first publish rather than a failure at render time.
*/}}
{{- define "aiwatcher.promptStoreSecretRef" -}}
{{- $ps := .Values.promptStore -}}
{{- if $ps.credentialsSecret.name -}}
name: {{ $ps.credentialsSecret.name }}
{{- else if eq $ps.mode "external" -}}
{{- fail "promptStore.mode is \"external\" but promptStore.credentialsSecret.name is empty. An external bucket needs credentials this chart cannot generate." -}}
{{- else -}}
name: {{ include "aiwatcher.fullname" . }}-rustfs
{{- end -}}
{{- end -}}

{{/*
The query engine this release runs, as one dict — enabled, engine, image,
resources, replicas, admission and scheduling — for every template that needs
it, so no two of them can come to disagree about which engine that is.

`query.*` is the only way in. `flow.*`, its name while Flow was the only engine,
was read for one release and is refused by `server.yaml` since.

The engine is checked here, at render time, for the reason the server refuses an
unknown one at start-up: `query.engine: polars` is a release whose values name
an engine no image implements.
*/}}
{{- define "aiwatcher.query" -}}
{{- $q := .Values.query -}}
{{- if not (has $q.engine (list "flow" "datafusion" "duckdb")) -}}
{{- fail (printf "query.engine is %q; the engines are flow, datafusion, duckdb." $q.engine) -}}
{{- end -}}
{{- if $q.enabled -}}
enabled: true
engine: {{ $q.engine }}
image: {{ get $q.images $q.engine | toJson }}
resources: {{ get $q.resources $q.engine | toJson }}
replicas: {{ $q.replicas }}
admission: {{ $q.admission }}
nodeSelector: {{ $q.nodeSelector | toJson }}
tolerations: {{ $q.tolerations | toJson }}
affinity: {{ $q.affinity | toJson }}
{{- else -}}
enabled: false
engine: {{ $q.engine }}
{{- end -}}
{{- end -}}

{{/*
What the panel's nginx proxies the query engine to, or empty for "there is none".

Empty is a first-class answer, not a missing value: nginx returns 503 there and
the Query tab reads that as "the engine is not running" and says so, which is
the degradation ADR_0008 designed for.

`query.enabled` is the normal way in and needs no URL — the Service is this
release's. `panel.queryUpstream` is for the other case: an engine running
somewhere this chart does not manage. It wins when both are set, because an
explicit URL is not something to silently ignore.
*/}}
{{- define "aiwatcher.queryUpstream" -}}
{{- $upstream := .Values.panel.queryUpstream -}}
{{- if $upstream -}}
{{- $upstream | trimSuffix "/" -}}
{{- else if (include "aiwatcher.query" . | fromYaml).enabled -}}
http://{{ include "aiwatcher.fullname" . }}-query:8081
{{- end -}}
{{- end -}}

{{/*
`host:port` of the workflow store's database, or empty for "there is none".

The same three modes as every other backend, and the same reason the failure is
at render time rather than at start-up: `execution.store: postgres` with
`postgresql.mode: none` is a release that would come up refusing every managed
run while its values file said managed execution was on.
*/}}
{{- define "aiwatcher.workflowPostgresEndpoint" -}}
{{- $pg := .Values.postgresql -}}
{{- if eq $pg.mode "install" -}}
{{ include "aiwatcher.fullname" . }}-postgres:5432
{{- else if eq $pg.mode "external" -}}
{{- if not $pg.external.host -}}
{{- fail "postgresql.mode is \"external\" but postgresql.external.host is empty. Set the database host, or set mode to \"install\"." -}}
{{- end -}}
{{ $pg.external.host }}:{{ $pg.external.port }}
{{- else if ne $pg.mode "none" -}}
{{- fail (printf "postgresql.mode is %q; it must be one of install, external, none." $pg.mode) -}}
{{- end -}}
{{- end -}}

{{/*
The Secret holding the database password.

An `external` database must name one, for the reason an external bucket must:
this chart cannot invent credentials for something somebody else owns, and
defaulting to the release's own Secret would fail at the first connection
rather than at render time.
*/}}
{{- define "aiwatcher.workflowPostgresSecretRef" -}}
{{- $pg := .Values.postgresql -}}
{{- if $pg.credentialsSecret.name -}}
name: {{ $pg.credentialsSecret.name }}
{{- else if eq $pg.mode "external" -}}
{{- fail "postgresql.mode is \"external\" but postgresql.credentialsSecret.name is empty. An external database needs a password this chart cannot generate." -}}
{{- else -}}
name: {{ include "aiwatcher.fullname" . }}-postgres
{{- end -}}
{{- end -}}

{{/*
Where a managed query step runs, or empty for "this deployment runs none".

Empty is a first-class answer here as it is for the panel's upstream, and it
means something sharper: a process with no address registers no query executor,
so it never *claims* a query attempt. A process that cannot do the work takes
none of it, rather than failing every attempt it takes.
*/}}
{{- define "aiwatcher.executionQueryUrl" -}}
{{- $url := .Values.execution.queryUrl -}}
{{- if $url -}}
{{- $url | trimSuffix "/" -}}
{{- else if (include "aiwatcher.query" . | fromYaml).enabled -}}
http://{{ include "aiwatcher.fullname" . }}-query:8081
{{- end -}}
{{- end -}}

{{/*
Everything both roles put in the environment.

Section 27 splits this binary in two — `serve` holds the API and the read
model, `work` holds the outbox and the reactors — and they are the *same
binary reading the same configuration*: the same store, the same log, the same
object store, the same identity provider. So the environment is written once
and rendered twice, and the two things that differ are rendered beside each
call: `AIWATCHER_ROLE`, and the listen address the work role has no use for
because it opens no socket.

Two copies of this list would be one release where a variable was added to the
API and not to the worker, which is a worker that runs with a different idea of
where the object store is — and every symptom of that appears three attempts
later as "holds no object".
*/}}
{{- define "aiwatcher.serverEnv" -}}
{{- $otlp := include "aiwatcher.otlpEndpoint" . -}}
{{- $promptStore := include "aiwatcher.promptStoreEndpoint" . -}}
- { name: AIWATCHER_BUS, value: {{ .Values.server.bus | quote }} }
- { name: AIWATCHER_DATA_DIR, value: /var/lib/aiwatcher }
- { name: AIWATCHER_SERVICE_NAME, value: {{ .Values.server.serviceName | quote }} }
- { name: AIWATCHER_PROCESSOR_ID, value: {{ .Values.server.processorId | quote }} }
- { name: AIWATCHER_INGEST_ENABLED, value: {{ .Values.server.ingestEnabled | quote }} }
- { name: AIWATCHER_LOG, value: {{ .Values.server.logLevel | quote }} }
- { name: AIWATCHER_LOG_FORMAT, value: {{ .Values.server.logFormat | quote }} }
- { name: AIWATCHER_MAX_RUNS, value: {{ .Values.server.maxRuns | quote }} }
- { name: AIWATCHER_MAX_SPANS_TOTAL, value: {{ .Values.server.maxSpansTotal | quote }} }
- { name: AIWATCHER_ORPHAN_TIMEOUT_SECONDS, value: {{ .Values.server.orphanTimeoutSeconds | quote }} }
{{- with .Values.server.corsOrigins }}
- { name: AIWATCHER_CORS_ORIGINS, value: {{ join "," . | quote }} }
{{- end }}
{{- if $otlp }}
- { name: AIWATCHER_OTLP_ENDPOINT, value: {{ $otlp | quote }} }
{{- end }}
{{- if $promptStore }}
- { name: AIWATCHER_PROMPT_STORE, value: "s3" }
- { name: AIWATCHER_PROMPT_S3_ENDPOINT, value: {{ $promptStore | quote }} }
- { name: AIWATCHER_PROMPT_S3_BUCKET, value: {{ .Values.promptStore.bucket | quote }} }
- { name: AIWATCHER_PROMPT_PREFIX, value: {{ .Values.promptStore.prefix | quote }} }
- { name: AIWATCHER_PROMPT_S3_REGION, value: {{ .Values.promptStore.external.region | quote }} }
- name: AIWATCHER_PROMPT_S3_CREATE_BUCKET
  value: {{ eq .Values.promptStore.mode "install" | ternary "true" (.Values.promptStore.external.createBucket | toString) | quote }}
- name: AIWATCHER_PROMPT_S3_ACCESS_KEY
  valueFrom:
    secretKeyRef:
      {{- include "aiwatcher.promptStoreSecretRef" . | nindent 6 }}
      key: {{ .Values.promptStore.credentialsSecret.accessKeyKey }}
- name: AIWATCHER_PROMPT_S3_SECRET_KEY
  valueFrom:
    secretKeyRef:
      {{- include "aiwatcher.promptStoreSecretRef" . | nindent 6 }}
      key: {{ .Values.promptStore.credentialsSecret.secretKeyKey }}
{{- else }}
# No store: every /api/v1/prompts route answers 501, and the panel
# says which variable is unset rather than showing an empty list.
- { name: AIWATCHER_PROMPT_STORE, value: "none" }
{{- end }}
{{- if .Values.conversationArchive.enabled }}
- { name: AIWATCHER_CONVERSATION_ARCHIVE, value: "on" }
- { name: AIWATCHER_CONVERSATION_PREFIX, value: {{ .Values.conversationArchive.prefix | quote }} }
- { name: AIWATCHER_CONVERSATION_POLICY, value: {{ .Values.conversationArchive.policy | quote }} }
- { name: AIWATCHER_CONVERSATION_MAX_TTL_DAYS, value: {{ .Values.conversationArchive.maxTtlDays | quote }} }
- { name: AIWATCHER_CONVERSATION_REJECT_ON_FINDING, value: {{ .Values.conversationArchive.rejectOnFinding | quote }} }
- { name: AIWATCHER_CONVERSATION_SWEEP_SECONDS, value: {{ .Values.conversationArchive.sweepSeconds | quote }} }
- name: AIWATCHER_CONVERSATION_KEYS
  valueFrom:
    secretKeyRef:
      name: {{ .Values.conversationArchive.keysSecret.name | quote }}
      key: {{ .Values.conversationArchive.keysSecret.key | quote }}
{{- end }}
- { name: AIWATCHER_AUTH_MODE, value: {{ .Values.auth.mode | quote }} }
{{- if ne .Values.auth.mode "none" }}
- { name: AIWATCHER_AUTH_PROVIDER_NAME, value: {{ .Values.auth.providerName | quote }} }
- { name: AIWATCHER_AUTH_ADMIN_GROUPS, value: {{ join "," .Values.auth.roles.adminGroups | quote }} }
- { name: AIWATCHER_AUTH_EDITOR_GROUPS, value: {{ join "," .Values.auth.roles.editorGroups | quote }} }
- { name: AIWATCHER_AUTH_VIEWER_GROUPS, value: {{ join "," .Values.auth.roles.viewerGroups | quote }} }
- { name: AIWATCHER_AUTH_DEFAULT_ROLE, value: {{ .Values.auth.roles.defaultRole | quote }} }
{{- with .Values.auth.roles.requiredGroups }}
- { name: AIWATCHER_AUTH_REQUIRED_GROUPS, value: {{ join "," . | quote }} }
{{- end }}
{{- with .Values.auth.ingestTokenSecret.name }}
- name: AIWATCHER_AUTH_INGEST_TOKENS
  valueFrom:
    secretKeyRef:
      name: {{ . }}
      key: {{ $.Values.auth.ingestTokenSecret.key }}
{{- end }}
{{- end }}
{{- if eq .Values.auth.mode "oidc" }}
- name: AIWATCHER_AUTH_ISSUER
  value: {{ required "auth.mode is \"oidc\" but auth.oidc.issuer is empty. For authentik it is https://<host>/application/o/<application-slug>/ — the application slug, not the provider name." .Values.auth.oidc.issuer | quote }}
- { name: AIWATCHER_AUTH_CLIENT_ID, value: {{ .Values.auth.oidc.clientId | quote }} }
- { name: AIWATCHER_AUTH_REDIRECT_URL, value: {{ include "aiwatcher.authRedirectUrl" . | quote }} }
- { name: AIWATCHER_AUTH_SCOPES, value: {{ join " " .Values.auth.oidc.scopes | quote }} }
- { name: AIWATCHER_AUTH_GROUPS_CLAIM, value: {{ .Values.auth.oidc.groupsClaim | quote }} }
- { name: AIWATCHER_AUTH_SESSION_TTL_SECONDS, value: {{ .Values.auth.oidc.sessionTtlSeconds | quote }} }
{{- with .Values.auth.oidc.postLogoutUrl }}
- { name: AIWATCHER_AUTH_POST_LOGOUT_URL, value: {{ . | quote }} }
{{- end }}
- name: AIWATCHER_AUTH_CLIENT_SECRET
  valueFrom:
    secretKeyRef:
      name: {{ include "aiwatcher.authSecretName" . }}
      key: {{ .Values.auth.oidc.secret.clientSecretKey }}
- name: AIWATCHER_AUTH_SESSION_SECRET
  valueFrom:
    secretKeyRef:
      name: {{ include "aiwatcher.authSecretName" . }}
      key: {{ .Values.auth.oidc.secret.sessionSecretKey }}
      # Optional so an installation can start without one and be
      # given one later; the cost until then is that a restart
      # signs everybody out.
      optional: true
{{- end }}
{{- if ne .Values.execution.store "none" }}
- { name: AIWATCHER_WORKFLOW_STORE, value: {{ .Values.execution.store | quote }} }
{{- if gt (int .Values.execution.retentionDays) 0 }}
- { name: AIWATCHER_WORKFLOW_RETENTION_DAYS, value: {{ .Values.execution.retentionDays | quote }} }
{{- end }}
# Sent whenever they differ from the binary's own defaults, so a chart that
# says nothing about them leaves the decision where it already was rather
# than restating it in a second place free to drift.
{{- if ne (int .Values.execution.maxAnswersPerStep) 256 }}
- { name: AIWATCHER_MAX_ANSWERS_PER_STEP, value: {{ .Values.execution.maxAnswersPerStep | quote }} }
{{- end }}
{{- if ne (int .Values.execution.maxAnswerBytes) 65536 }}
- { name: AIWATCHER_MAX_ANSWER_BYTES, value: {{ .Values.execution.maxAnswerBytes | quote }} }
{{- end }}
{{- if eq .Values.execution.store "postgres" }}
# The password reaches the DSN through the variable above it, which
# Kubernetes expands in `value` — so it is in a Secret rather than
# in the manifest, and the connection string is still one string the
# server parses.
- name: AIWATCHER_WORKFLOW_POSTGRES_PASSWORD
  valueFrom:
    secretKeyRef:
      {{- include "aiwatcher.workflowPostgresSecretRef" . | nindent 6 }}
      key: {{ .Values.postgresql.credentialsSecret.passwordKey }}
- name: AIWATCHER_WORKFLOW_POSTGRES_URL
  value: "postgres://{{ .Values.postgresql.username }}:$(AIWATCHER_WORKFLOW_POSTGRES_PASSWORD)@{{ include "aiwatcher.workflowPostgresEndpoint" . }}/{{ .Values.postgresql.database }}"
- { name: AIWATCHER_WORKFLOW_POSTGRES_MAX_CONNECTIONS, value: {{ .Values.postgresql.maxConnections | quote }} }
{{- end }}
{{- with include "aiwatcher.executionQueryUrl" . }}
- { name: AIWATCHER_QUERY_URL, value: {{ . | quote }} }
{{- end }}
# The engine a plan has to be written for to start here, which is the engine
# `query.engine` runs — so switching a release is one value, and the server
# follows it.
- { name: AIWATCHER_QUERY_ENGINE, value: {{ (include "aiwatcher.query" . | fromYaml).engine | quote }} }
# A step's clock, which the engine's ceiling is derived a minute above
# (`aiwatcher.queryTimeoutSeconds`), so the step's is the one that stops first.
- { name: AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS, value: {{ .Values.execution.queryStepTimeoutSeconds | int | quote }} }
{{- with .Values.execution.mlPipelineUrl }}
- { name: AIWATCHER_ML_PIPELINE_URL, value: {{ . | quote }} }
{{- end }}
{{- end }}
{{- if eq .Values.server.bus "laser" }}
- name: AIWATCHER_LASER_CONNECTION_STRING
  valueFrom:
    secretKeyRef:
      name: {{ .Values.server.laser.connectionStringSecret.name }}
      key: {{ .Values.server.laser.connectionStringSecret.key }}
- { name: AIWATCHER_LASER_STREAM, value: {{ .Values.server.laser.stream | quote }} }
- { name: AIWATCHER_LASER_TOPIC, value: {{ .Values.server.laser.topic | quote }} }
{{- end }}
{{- end -}}

{{- define "aiwatcher.imagePullSecrets" -}}
{{- with .Values.imagePullSecrets }}
imagePullSecrets:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- end -}}

{{/*
Where authentik sends the browser back to.

Derived from the ingress host rather than configured twice, because it has to
match the redirect URI registered on the authentik application byte for byte
and a second copy of one string is a second chance to get it wrong. Naming it
explicitly still wins — a release published on a host this chart does not own
has no way to derive it.
*/}}
{{- define "aiwatcher.authRedirectUrl" -}}
{{- $auth := .Values.auth -}}
{{- if $auth.oidc.redirectUrl -}}
{{- $auth.oidc.redirectUrl -}}
{{- else if .Values.ingress.enabled -}}
{{- $scheme := .Values.ingress.tls.enabled | ternary "https" "http" -}}
{{- printf "%s://%s/api/v1/auth/callback" $scheme (required "auth.mode is \"oidc\" and ingress.enabled is true, but ingress.host is empty" .Values.ingress.host) -}}
{{- else -}}
{{- fail "auth.mode is \"oidc\" but there is no ingress to derive a redirect URL from. Set auth.oidc.redirectUrl to the absolute URL authentik will redirect to, which must match the application's redirect URI exactly." -}}
{{- end -}}
{{- end -}}

{{/*
The Secret holding the client secret and the session signing key.

Required, the same way the Laser connection string is: both are passwords, and
a chart that let them come from values would put them in every `helm get
values` and every CI log that renders the release.
*/}}
{{- define "aiwatcher.authSecretName" -}}
{{- $name := .Values.auth.oidc.secret.name -}}
{{- if not $name -}}
{{- fail "auth.mode is \"oidc\" but auth.oidc.secret.name is empty. It holds the client secret and the session signing key, both of which are passwords, so they come from a Secret rather than from values." -}}
{{- end -}}
{{- $name -}}
{{- end -}}

{{/*
The query engine's ceiling on one query, or nothing for the engine's own default
(30 s, sized for a person at the Query tab). `query.timeoutSeconds` when set; with
managed execution on, a minute above `execution.queryStepTimeoutSeconds`, so a
long step is stopped by its own clock and retried, rather than refused by the
engine with a 422 that fails it outright. AW-3's spec asks that the engine's
ceiling stay above the step's, and the two live in two processes, so this is the
one place that can keep them in order — which is why a `query.timeoutSeconds`
at or below the step's is refused rather than rendered. `int` throughout, because
a value from `--set` is a float64 and one of a million renders as `1e+06`.
*/}}
{{- define "aiwatcher.queryTimeoutSeconds" -}}
{{- $own := int .Values.query.timeoutSeconds -}}
{{- $step := int .Values.execution.queryStepTimeoutSeconds -}}
{{- $managed := ne .Values.execution.store "none" -}}
{{- if and $managed (gt $own 0) (le $own $step) -}}
{{- fail (printf "query.timeoutSeconds is %d, at or below execution.queryStepTimeoutSeconds (%d). The engine would refuse a long step with a 422 before the step's own clock stops it, which fails the step outright instead of retrying it. Raise query.timeoutSeconds above the step's, or leave it at 0 to derive it a minute above." $own $step) -}}
{{- end -}}
{{- if gt $own 0 -}}
{{ $own }}
{{- else if $managed -}}
{{ add $step 60 }}
{{- end -}}
{{- end -}}
