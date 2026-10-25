#!/bin/sh
# Your own Tetrad updates (docs/development.md, "Your own updates"). The unit installs only a
# manifest that the key built into its Tetrad signed. This makes your own key, and signs the
# files of a build with it, so that the Remote page (a dev build) or `tetrad-usb install` can
# send them. It does what the release workflow does, with your key.
#
#   dev-update.sh key KEY.pem
#       Makes the private key KEY.pem when it is not there, and prints the public key. Build
#       Tetrad with it: TETRAD_RELEASE_PUBKEY=<public key> cargo ... (or in build-arm.sh's
#       environment). Keep KEY.pem; nobody else needs it.
#   dev-update.sh pack DIR KEY.pem
#       DIR holds the `tetrad` binary of the build. Adds the three launcher scripts,
#       tetrad-update.json (the version of Cargo.toml, and the size and SHA-256 of each file) and
#       tetrad-update.json.sig (its Ed25519 signature). Send the six files.
#   dev-update.sh archive BINARY [--scripts] [OUT]
#       Makes OUT (default tetrad-update.tar.gz, beside BINARY): one file with the `tetrad`
#       binary, and with --scripts the three launcher scripts of this checkout. This is what a
#       dev build takes over Wi-Fi: unsigned, so it needs no key. scripts/build-dev.sh makes it
#       after each build.
#   dev-update.sh push ARCHIVE HOST PIN
#       Sends the files of ARCHIVE to the Remote page of a dev build, as the page does: HOST is
#       the IP address of the unit, PIN the one that Settings > System > Remote shows. Touch
#       UPDATE TETRAD on the unit after it.
#   dev-update.sh log HOST PIN
#       Prints the recent notices of the unit (the log), over the same Remote page of a dev
#       build.
#
# `key` and `pack` are for builds with their own signing key, which `tetrad-usb install` takes
# over USB; `archive` and `push` need no key.
# Needs OpenSSL 3 (key, pack), base64, sha256sum or shasum (pack), tar, and curl (push).
set -eu

here=$(cd -- "$(dirname -- "$0")" && pwd)
repo=$(cd -- "$here/.." && pwd)
scripts="tetrad-launch.sh tetrad-launcher.sh tetrad-update.sh"

fail() { echo "dev-update: $*" >&2; exit 1; }
usage() { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -c1-64
    else shasum -a 256 "$1" | cut -c1-64; fi
}
size() { wc -c <"$1" | tr -d ' '; }
b64() { base64 | tr -d '\n'; }

public_key() {
    openssl pkey -in "$1" -pubout -outform DER | tail -c 32 | b64
}

# One request to the Remote page: call METHOD PATH [BODY-FILE]. The body of the answer is in
# $reply; a status other than 200 stops with the reason that the unit gives.
call() {
    status=$(curl -sS -m 30 -o "$reply" -w '%{http_code}' -X "$1" -H "X-Tetrad-Pin: $pin" \
        ${3:+--data-binary "@$3"} "http://$host$2") || fail "the unit at $host is not reachable"
    if [ "$status" != 200 ]; then
        fail "$2: $status $(sed -n 's/.*"error":"\([^"]*\)".*/\1/p' "$reply")"
    fi
}

case ${1:-} in
    key)
        [ $# -eq 2 ] || usage
        key=$2
        if [ ! -f "$key" ]; then
            (umask 077 && openssl genpkey -algorithm ed25519 -out "$key") || fail "cannot make $key"
            echo "made $key (keep it private)" >&2
        fi
        pub=$(public_key "$key")
        echo "public key: $pub"
        echo "build with: TETRAD_RELEASE_PUBKEY=$pub"
        ;;
    pack)
        [ $# -eq 3 ] || usage
        dir=$2
        key=$3
        [ -f "$dir/tetrad" ] || fail "no $dir/tetrad (build it first)"
        [ -f "$key" ] || fail "no $key (dev-update.sh key $key makes it)"
        version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml")
        [ -n "$version" ] || fail "no version in $repo/Cargo.toml"
        for s in $scripts; do
            install -m 755 "$repo/scripts/launcher/$s" "$dir/$s"
        done
        # The files of a manifest in sorted order: the binary, then the scripts.
        entry() { printf '"%s":{"size":%s,"sha256":"%s"}' "$1" "$(size "$2")" "$(sha256 "$2")"; }
        {
            printf '{"version":"%s","files":{' "$version"
            entry bin/tetrad "$dir/tetrad"
            for s in $scripts; do
                printf ','
                entry "launcher/$s" "$dir/$s"
            done
            printf '}}'
        } >"$dir/tetrad-update.json"
        openssl pkeyutl -sign -inkey "$key" -rawin -in "$dir/tetrad-update.json" | b64 \
            >"$dir/tetrad-update.json.sig"
        echo "tetrad $version: signed with the key $(public_key "$key")" >&2
        echo "send these files of $dir: tetrad $scripts tetrad-update.json tetrad-update.json.sig" >&2
        ;;
    log)
        [ $# -eq 3 ] || usage
        host=$2
        pin=$3
        command -v curl >/dev/null || fail "log needs curl"
        reply=$(mktemp)
        trap 'rm -f "$reply"' EXIT
        call GET /api/log
        cat "$reply"
        ;;
    archive)
        [ $# -ge 2 ] || usage
        binary=$2
        shift 2
        with_scripts=
        out=
        for arg in "$@"; do
            case $arg in
                --scripts) with_scripts=yes ;;
                *) [ -z "$out" ] || usage; out=$arg ;;
            esac
        done
        [ -f "$binary" ] || fail "no $binary"
        [ -n "$out" ] || out=$(cd -- "$(dirname -- "$binary")" && pwd)/tetrad-update.tar.gz
        tmp=$(mktemp -d)
        trap 'rm -rf "$tmp"' EXIT
        install -m 755 "$binary" "$tmp/tetrad"
        names=tetrad
        if [ -n "$with_scripts" ]; then
            for s in $scripts; do
                install -m 755 "$repo/scripts/launcher/$s" "$tmp/$s"
            done
            names="tetrad $scripts"
        fi
        # shellcheck disable=SC2086 # names is a list of file names without spaces
        tar -C "$tmp" --owner=0 --group=0 --numeric-owner -czf "$out" $names
        echo "$out: $names ($(size "$out") bytes)" >&2
        ;;
    push)
        [ $# -eq 4 ] || usage
        archive=$2
        host=$3
        pin=$4
        command -v curl >/dev/null || fail "push needs curl"
        [ -f "$archive" ] || fail "no $archive"
        tmp=$(mktemp -d)
        trap 'rm -rf "$tmp"' EXIT
        reply=$tmp/reply
        chunk=$tmp/chunk
        mkdir "$tmp/files"
        tar -C "$tmp/files" -xzf "$archive" || fail "$archive is not a tar.gz"
        # The four update files by their base names; the unit takes no other.
        list=
        for name in tetrad $scripts; do
            [ -f "$tmp/files/$name" ] || continue
            case $name in tetrad) path=bin/tetrad ;; *) path=launcher/$name ;; esac
            list="$list $path"
        done
        [ -n "$list" ] || fail "$archive has none of: tetrad $scripts"
        call GET /api/update
        echo "the unit runs Tetrad $(sed -n 's/.*"version":"\([^"]*\)".*/\1/p' "$reply")" >&2
        sizes=
        for path in $list; do
            sizes="$sizes${sizes:+,}\"$path\":$(size "$tmp/files/${path##*/}")"
        done
        printf '{"files":{%s}}' "$sizes" >"$tmp/begin"
        call POST /api/update/begin "$tmp/begin"
        for path in $list; do
            file=$tmp/files/${path##*/}
            total=$(size "$file")
            offset=0
            while [ "$offset" -lt "$total" ]; do
                dd if="$file" of="$chunk" bs=49152 skip=$((offset / 49152)) count=1 2>/dev/null
                call POST "/api/update/chunk?path=$(echo "$path" | sed 's|/|%2F|')&offset=$offset" "$chunk"
                offset=$((offset + $(size "$chunk")))
                printf '\r%s %s/%s' "${path##*/}" "$offset" "$total" >&2
            done
            echo >&2
        done
        call POST /api/update/ready
        echo "staged; touch UPDATE TETRAD on the unit (Settings > System > Tetrad Updates)" >&2
        ;;
    *) usage ;;
esac
