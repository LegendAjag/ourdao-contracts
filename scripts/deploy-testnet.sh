#!/usr/bin/env bash
#
# Build the OurDAO contract, deploy it, and initialize it in one invocation, so
# a contract is never left deployed-but-uninitialized (`initialize` is a
# separate transaction and could otherwise be front-run).
#
# Testnet is the default. Any other network is refused unless you explicitly
# opt in, and then needs an interactive confirmation that --yes cannot skip.
#
# Usage:
#   ./scripts/deploy-testnet.sh [options] [identity-name]
#
# Options:
#   --dry-run             Print every command instead of running it. Builds,
#                         deploys and writes nothing, and needs no network.
#   --yes                 Skip the confirmation prompt (testnet only).
#   --network <name>      Network to deploy to (default: testnet).
#   --allow-non-testnet   Required for any network other than testnet.
#   -h, --help            Show this help.
#
# Environment overrides:
#   IDENTITY             stellar keys identity to deploy from   (default: ourdao-deployer)
#   NETWORK              same as --network                       (default: testnet)
#   ALIAS                local alias for the contract id         (default: ourdao-dao)
#   TOKEN                token contract id the DAO uses          (testnet default: native XLM SAC;
#                                                                required elsewhere)
#   ADMINS_JSON          JSON array of admin addresses           (default: the deployer)
#   CONSENSUS_THRESHOLD  loan approval threshold in bps          (default: 5100)
#   MEMBERSHIP_FEE       membership fee in token base units      (default: 10000000)
#   POLICY_JSON          LoanPolicy as JSON                      (default: 3-day edit/vote, 5-20% rates)
#
# On testnet the identity is created and funded via friendbot if it does not
# exist. On any other network the identity must already exist: this script
# never generates or funds keys there.
set -euo pipefail

IDENTITY="${IDENTITY:-ourdao-deployer}"
NETWORK="${NETWORK:-testnet}"
ALIAS="${ALIAS:-ourdao-dao}"
TOKEN="${TOKEN:-}"
ADMINS_JSON="${ADMINS_JSON:-}"
CONSENSUS_THRESHOLD="${CONSENSUS_THRESHOLD:-5100}"
MEMBERSHIP_FEE="${MEMBERSHIP_FEE:-10000000}"
POLICY_JSON="${POLICY_JSON:-}"
WASM_TARGET="wasm32v1-none"
if [[ -z "${POLICY_JSON}" ]]; then
  POLICY_JSON="$(printf '{"min_membership_duration":0,"membership_contribution":%s,"max_loan_duration":2592000,"min_interest_rate":500,"max_interest_rate":2000,"cooldown_period":0,"max_loan_to_treasury_ratio":5000,"default_grace_period":259200,"default_penalty_bps":2000,"editing_period":259200,"voting_period":259200,"treasury_threshold":6000,"quorum_bps":0}' "${MEMBERSHIP_FEE}")"
fi
WASM="target/${WASM_TARGET}/release/ourdao_dao.optimized.wasm"

DRY_RUN=0
ASSUME_YES=0
ALLOW_NON_TESTNET=0

usage() {
  sed -n '2,/^set -euo pipefail/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

die() {
  echo "error: $*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --yes) ASSUME_YES=1 ;;
    --allow-non-testnet) ALLOW_NON_TESTNET=1 ;;
    --network)
      [[ $# -ge 2 ]] || die "--network needs a value"
      NETWORK="$2"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*) die "unknown option: $1 (see --help)" ;;
    *) IDENTITY="$1" ;;
  esac
  shift
done

# Run from the repo root regardless of where the script is invoked.
cd "$(dirname "$0")/.."

IS_TESTNET=0
[[ "${NETWORK}" == "testnet" ]] && IS_TESTNET=1

if [[ ${IS_TESTNET} -eq 0 && ${ALLOW_NON_TESTNET} -eq 0 ]]; then
  die "refusing to deploy to '${NETWORK}': this script targets testnet. Pass --allow-non-testnet to opt in (and read the mainnet notes in the README first)."
fi

print_cmd() {
  printf '+'
  printf ' %q' "$@"
  printf '\n'
}

# run <cmd...>: execute, or just print under --dry-run.
run() {
  if [[ ${DRY_RUN} -eq 1 ]]; then
    print_cmd "$@"
  else
    "$@"
  fi
}

# capture <var> <dry-run placeholder> <cmd...>: store the command's stdout in <var>.
capture() {
  local var="$1" placeholder="$2"
  shift 2
  if [[ ${DRY_RUN} -eq 1 ]]; then
    print_cmd "$@"
    printf -v "${var}" '%s' "${placeholder}"
  else
    printf -v "${var}" '%s' "$("$@")"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if [[ ${DRY_RUN} -eq 1 ]]; then
  echo "==> DRY RUN: commands are printed, not executed. Nothing is built, deployed or written."
else
  command -v stellar >/dev/null 2>&1 ||
    die "the 'stellar' CLI is not installed. See https://developers.stellar.org/docs/tools/developer-tools/cli/stellar-cli"
fi

echo "==> Checking identity '${IDENTITY}' on ${NETWORK}"
if [[ ${DRY_RUN} -eq 1 ]]; then
  if [[ ${IS_TESTNET} -eq 1 ]]; then
    echo "    (would create and friendbot-fund '${IDENTITY}' if it does not exist)"
  else
    echo "    (would require '${IDENTITY}' to already exist; keys are never generated on ${NETWORK})"
  fi
  DEPLOYER_ADDR="<deployer-address>"
elif stellar keys address "${IDENTITY}" >/dev/null 2>&1; then
  if [[ ${IS_TESTNET} -eq 1 ]]; then
    # Top up in case of a testnet reset.
    stellar keys fund "${IDENTITY}" --network "${NETWORK}" || true
  fi
  DEPLOYER_ADDR="$(stellar keys address "${IDENTITY}")"
elif [[ ${IS_TESTNET} -eq 1 ]]; then
  stellar keys generate "${IDENTITY}" --network "${NETWORK}" --fund
  DEPLOYER_ADDR="$(stellar keys address "${IDENTITY}")"
else
  die "identity '${IDENTITY}' does not exist. Create or import it yourself before deploying to ${NETWORK}."
fi
echo "    deployer address: ${DEPLOYER_ADDR}"

if [[ -z "${ADMINS_JSON}" ]]; then
  ADMINS_JSON="[\"${DEPLOYER_ADDR}\"]"
fi

if [[ -z "${TOKEN}" ]]; then
  if [[ ${IS_TESTNET} -eq 1 ]]; then
    capture TOKEN "<native-asset-contract-id>" \
      stellar contract id asset --asset native --network "${NETWORK}"
  else
    die "TOKEN must be set explicitly for ${NETWORK} (the contract id of the asset the DAO holds)."
  fi
fi

echo "==> Building optimized wasm"
run stellar contract build --optimize
if [[ ${DRY_RUN} -eq 1 ]]; then
  WASM_SHA256="<sha256 of ${WASM}, computed after the build>"
else
  ls -la "${WASM}"
  WASM_SHA256="$(sha256_of "${WASM}")"
fi
echo "    SHA-256: ${WASM_SHA256}"

echo ""
echo "==================================================================="
echo " About to deploy AND initialize OurDAO"
echo "   network:             ${NETWORK}"
echo "   deployer:            ${DEPLOYER_ADDR}"
echo "   wasm:                ${WASM}"
echo "   wasm sha256:         ${WASM_SHA256}"
echo "   token:               ${TOKEN}"
echo "   admins:              ${ADMINS_JSON}"
echo "   consensus threshold: ${CONSENSUS_THRESHOLD} bps"
echo "   membership fee:      ${MEMBERSHIP_FEE}"
echo "   policy:              ${POLICY_JSON}"
echo "   alias:               ${ALIAS}"
echo "==================================================================="

if [[ ${DRY_RUN} -eq 0 ]]; then
  if [[ ${IS_TESTNET} -eq 1 ]]; then
    if [[ ${ASSUME_YES} -eq 0 ]]; then
      read -r -p "Deploy this wasm to testnet? [y/N] " reply
      [[ "${reply}" == "y" || "${reply}" == "Y" || "${reply}" == "yes" ]] || die "aborted"
    fi
  else
    # --yes deliberately does not skip this: a non-testnet deploy is irreversible.
    [[ -t 0 ]] || die "a non-testnet deploy needs an interactive terminal for confirmation"
    echo "This is NOT testnet, and the deployed contract cannot be upgraded."
    read -r -p "Type the network name (${NETWORK}) to confirm: " reply
    [[ "${reply}" == "${NETWORK}" ]] || die "aborted"
  fi
fi

echo "==> Deploying to ${NETWORK}"
capture CONTRACT_ID "<contract-id>" \
  stellar contract deploy \
  --wasm "${WASM}" \
  --source "${IDENTITY}" \
  --network "${NETWORK}" \
  --alias "${ALIAS}"

echo "==> Initializing ${CONTRACT_ID}"
if ! run stellar contract invoke \
  --id "${CONTRACT_ID}" \
  --source "${IDENTITY}" \
  --network "${NETWORK}" \
  -- \
  initialize \
  --admins "${ADMINS_JSON}" \
  --consensus_threshold "${CONSENSUS_THRESHOLD}" \
  --membership_fee "${MEMBERSHIP_FEE}" \
  --token "${TOKEN}" \
  --policy "${POLICY_JSON}"; then
  echo "" >&2
  echo "error: ${CONTRACT_ID} is DEPLOYED BUT NOT INITIALIZED." >&2
  echo "       Anyone can initialize it until you do. Re-run the 'initialize' invocation above" >&2
  echo "       immediately, or treat this contract id as compromised and deploy a new one." >&2
  exit 1
fi

echo ""
echo "==================================================================="
if [[ ${DRY_RUN} -eq 1 ]]; then
  echo " Dry run complete: nothing was deployed."
else
  echo " Deployed and initialized OurDAO on ${NETWORK}"
  echo "   contract id: ${CONTRACT_ID}"
  echo "   saved alias: ${ALIAS}"
  echo "   wasm sha256: ${WASM_SHA256}"
fi
echo "==================================================================="
echo ""
echo "To verify this deployment against a release, see DEPLOYMENTS.md"
