#!/bin/sh
# An operator's lifecycle hook for a node: sends everything a unit reaches outside the host through a VPN tunnel of
# the host, and drops it, rather than letting it out another way, while the tunnel is down.
#
# Name this one file for both moments in the node's machine policy:
#
#   orch-node policy set hooks '{"post_up":"/etc/orch/hooks/route-through-vpn.sh","pre_down":"/etc/orch/hooks/route-through-vpn.sh"}'
#
# It reads which moment it is from the JSON document on its standard input. It can be run any number of times for
# the same unit (a node runs post_up again for the units it finds running when it starts): it replaces what it
# added and never adds a second copy.
#
# What leaves the host from a unit leaves from its own subnet: an open unit's network of its own, or for a restricted
# unit the network its proxy goes out on. For that subnet it adds two rules:
#   1. the host's own specific routes (its networks, Docker's networks, Orch's services) are used as they are;
#   2. everything else is looked up in a table of its own, whose default route is the tunnel and, below it, an
#      unreachable route, so that while the tunnel is down nothing goes out at all.
#
# Needs, where the hook runs: `jq`, and either a Docker client with the node's Docker socket (the bundled node runs
# in a container without the right to change the host's routes, so the routes are changed by a short-lived helper
# container on the host's network) or, for a node installed on the host itself, `ip` and the right to change routes
# (set DIRECT below). A hook is given no variable of the node's, so its settings are here.
set -eu

DEVICE="wg0"                 # the tunnel's interface on the host
TABLE="200"                  # a routing table of its own for the units' traffic
MAIN_PRIORITY="20000"        # the rule that keeps the host's specific routes
TUNNEL_PRIORITY="20001"      # the rule that sends the rest into the table
HELPER_IMAGE="orch-route-helper"   # an image with iproute2's `ip` (busybox's lacks options used here), on the host
DIRECT="no"                  # "yes" where the node runs on the host with the right to change routes

document=$(cat)
event=$(printf '%s' "$document" | jq -r '.event')
# The subnet traffic leaves from: a restricted unit's proxy's way out where it has one, else the unit's own network.
subnets=$(printf '%s' "$document" | jq -r '
  [.network.networks[] | select(.role == "egress") | .subnet] as $out
  | if ($out | length) > 0 then $out[] else (.network.networks[] | select(.role == "unit") | .subnet) end')

routes() {
    if [ "$DIRECT" = "yes" ]; then
        sh -c "$1"
    else
        docker run --rm --pull=never --network host --cap-add NET_ADMIN "$HELPER_IMAGE" sh -c "$1"
    fi
}

script=""
for subnet in $subnets; do
    # Every rule is taken away before it is added, so a run again leaves one copy.
    remove="ip rule del from $subnet lookup main suppress_prefixlength 0 priority $MAIN_PRIORITY 2>/dev/null || true;
ip rule del from $subnet lookup $TABLE priority $TUNNEL_PRIORITY 2>/dev/null || true;"
    case "$event" in
        post_up)
            script="$script $remove
ip route replace default dev $DEVICE table $TABLE 2>/dev/null || true;
ip route replace unreachable default metric 4294967295 table $TABLE;
ip rule add from $subnet lookup main suppress_prefixlength 0 priority $MAIN_PRIORITY;
ip rule add from $subnet lookup $TABLE priority $TUNNEL_PRIORITY;"
            ;;
        pre_down)
            script="$script $remove"
            ;;
        *)
            ;;
    esac
done

if [ -n "$script" ]; then
    routes "set -e; $script"
fi
# A post_up that could not put its rules in place exits other than 0 (set -e), and the unit does not run.
