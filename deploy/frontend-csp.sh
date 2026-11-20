#!/bin/sh
# frontend-csp.sh [SERVICE]: the content policy's room for the service of the human check, as nginx variables.
#
# The web edge sends one content policy on every response. A human check loads a service's script and frames
# from outside, so its hosts must be named in the policy; with none chosen the variables are empty and the
# policy is exactly the one the edge always sent. SERVICE is ORCH_FRONTEND_HUMAN_CHECK:
#   (empty)          nothing is added
#   hcaptcha         hCaptcha: its hosts in script-src, frame-src, style-src and connect-src
#   recaptcha        reCAPTCHA of Google Cloud Fraud Defense, from www.google.com
#   recaptcha-net    the same through www.recaptcha.net, where www.google.com cannot be reached
# A frame-src is written only when a service needs frames; it keeps 'self', which frames fell back to before.
# The hosts are the ones each service's own documentation names; docs/operator/human-check.md has the table.
set -eu
service=${1:-}
script=""
style=""
connect=""
frame=""
case "$service" in
  "") ;;
  hcaptcha)
    hosts="https://hcaptcha.com https://*.hcaptcha.com"
    script=" $hosts"
    style=" $hosts"
    connect=" $hosts"
    frame="; frame-src 'self' $hosts"
    ;;
  recaptcha)
    script=" https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/"
    connect=" https://www.google.com/recaptcha/"
    frame="; frame-src 'self' https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/"
    ;;
  recaptcha-net)
    script=" https://www.recaptcha.net/recaptcha/ https://www.gstatic.com/recaptcha/"
    connect=" https://www.recaptcha.net/recaptcha/"
    frame="; frame-src 'self' https://www.recaptcha.net/recaptcha/ https://recaptcha.google.com/recaptcha/"
    ;;
  *)
    echo "ORCH_FRONTEND_HUMAN_CHECK is '$service': it must be empty, hcaptcha, recaptcha or recaptcha-net" >&2
    exit 1
    ;;
esac
printf 'set $csp_script "%s";\nset $csp_style "%s";\nset $csp_connect "%s";\nset $csp_frame "%s";\n' \
  "$script" "$style" "$connect" "$frame"
