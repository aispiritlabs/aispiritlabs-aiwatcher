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

{{- define "aiwatcher.imagePullSecrets" -}}
{{- with .Values.imagePullSecrets }}
imagePullSecrets:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- end -}}
