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
What the panel's nginx proxies /flow to, or empty for "there is none".

Empty is a first-class answer, not a missing value: nginx returns 503 there and
the Query tab reads that as "the service is not running" and says so, which is
the degradation ADR_0008 designed for.

`flow.enabled` is the normal way in and needs no URL — the Service is this
release's. `panel.flowUpstream` stays for the other case: a Flow service running
somewhere this chart does not manage. It wins when both are set, because an
explicit URL is not something to silently ignore.
*/}}
{{- define "aiwatcher.flowUpstream" -}}
{{- if .Values.panel.flowUpstream -}}
{{- .Values.panel.flowUpstream | trimSuffix "/" -}}
{{- else if .Values.flow.enabled -}}
http://{{ include "aiwatcher.fullname" . }}-flow:8081
{{- end -}}
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
