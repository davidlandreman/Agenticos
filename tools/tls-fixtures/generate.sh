#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/agenticos-tls.XXXXXX")
cleanup() {
    status=$?
    rm -rf "$work_dir"
    exit "$status"
}
trap cleanup EXIT HUP INT TERM

make_ca() {
    ca_dir=$1
    common_name=$2
    mkdir -p "$ca_dir/newcerts"
    : > "$ca_dir/index.txt"
    printf '1000\n' > "$ca_dir/serial"
    openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
        -subj "/CN=$common_name" \
        -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
        -addext 'keyUsage=critical,keyCertSign,cRLSign' \
        -keyout "$ca_dir/root.key" -out "$ca_dir/root.pem" >/dev/null 2>&1
}

sign_leaf() {
    ca_dir=$1
    name=$2
    common_name=$3
    san=$4
    not_before=$5
    not_after=$6
    openssl req -new -newkey rsa:2048 -nodes -sha256 \
        -subj "/CN=$common_name" -addext "subjectAltName=$san" \
        -addext 'basicConstraints=critical,CA:FALSE' \
        -addext 'keyUsage=critical,digitalSignature,keyEncipherment' \
        -addext 'extendedKeyUsage=serverAuth' \
        -keyout "$work_dir/$name.key" -out "$work_dir/$name.csr" >/dev/null 2>&1
    sed \
        -e "s|@DATABASE@|$ca_dir/index.txt|" \
        -e "s|@NEW_CERTS@|$ca_dir/newcerts|" \
        -e "s|@CERTIFICATE@|$ca_dir/root.pem|" \
        -e "s|@PRIVATE_KEY@|$ca_dir/root.key|" \
        -e "s|@SERIAL@|$ca_dir/serial|" \
        "$script_dir/openssl-test.cnf" > "$work_dir/signing.cnf"
    openssl ca -batch -notext -config "$work_dir/signing.cnf" \
        -startdate "$not_before" -enddate "$not_after" \
        -in "$work_dir/$name.csr" -out "$work_dir/$name.pem" >/dev/null
}

trusted="$work_dir/trusted"
untrusted="$work_dir/untrusted"
make_ca "$trusted" 'AgenticOS hermetic TLS test root'
make_ca "$untrusted" 'AgenticOS untrusted TLS test root'

valid_from=20260718000000Z
valid_until=20360718000000Z
sign_leaf "$trusted" valid valid.agenticos.test \
    'DNS:valid.agenticos.test,DNS:tls12.agenticos.test,IP:10.0.2.102' \
    "$valid_from" "$valid_until"
sign_leaf "$trusted" expired expired.agenticos.test \
    'DNS:expired.agenticos.test' 20250718000000Z 20260718000000Z
sign_leaf "$trusted" future future.agenticos.test \
    'DNS:future.agenticos.test' 20260720000000Z 20360720000000Z
sign_leaf "$untrusted" untrusted untrusted.agenticos.test \
    'DNS:untrusted.agenticos.test' "$valid_from" "$valid_until"

for name in valid expired future untrusted; do
    cp "$work_dir/$name.pem" "$script_dir/$name.pem"
    cp "$work_dir/$name.key" "$script_dir/$name.key"
done
cp "$trusted/root.pem" "$script_dir/root.pem"

echo "Regenerated hermetic TLS certificates in $script_dir"
