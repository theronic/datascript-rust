#!/bin/bash
# Compiles the conformance oracle: ClojureScript DataScript itself (this repository's src/) with
# conformance/oracle/src, into conformance/oracle/target/oracle.js, for Node.
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/../.."

CLOJURESCRIPT="${CLOJURESCRIPT:-1.12.145}"
DEPS="{:paths [\"src\" \"conformance/oracle/src\"]
       :deps {org.clojure/clojurescript {:mvn/version \"$CLOJURESCRIPT\"}
              persistent-sorted-set/persistent-sorted-set {:mvn/version \"0.3.1\"}
              io.github.tonsky/extend-clj {:mvn/version \"0.1.0\"}}}"

mkdir -p conformance/oracle/target
clojure -Sdeps "$DEPS" -M -m cljs.main \
  -t node -O simple \
  -d conformance/oracle/target/out \
  -o conformance/oracle/target/oracle.js \
  -c oracle.core
echo "oracle: conformance/oracle/target/oracle.js ($(wc -c < conformance/oracle/target/oracle.js | tr -d ' ') bytes)"
