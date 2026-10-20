#!/usr/bin/env bash
# Container entrypoint. Maps the Home Assistant add-on options in /data/options.json onto Kessel's
# environment variables; an environment variable that is already set always wins.
set -euo pipefail

: "${KESSEL_TOKEN_FILE:=/data/kessel.token}"
: "${KESSEL_CONFIG_FILE:=/data/kessel-config.json}"
: "${KESSEL_PROFILE_DIR:=/data/profiles}"
export KESSEL_TOKEN_FILE KESSEL_CONFIG_FILE KESSEL_PROFILE_DIR

read_option() {
    local key="$1" default="$2" value=""
    if [ -f /data/options.json ]; then
        value="$(jq -r --arg k "${key}" '.[$k] // empty' /data/options.json 2> /dev/null || true)"
    fi
    if [ -z "${value}" ] || [ "${value}" = "null" ]; then
        value="${default}"
    fi
    printf '%s' "${value}"
}

: "${KESSEL_LOG:=$(read_option log_level info)}"
: "${KESSEL_MQTT_AUTODISCOVER:=$(read_option mqtt_autodiscovery true)}"
export KESSEL_LOG KESSEL_MQTT_AUTODISCOVER

exec /usr/local/bin/kessel
