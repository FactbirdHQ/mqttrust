#!/usr/bin/env bash

if [[ -z "$DEVICE_ADVISOR_PASSWORD" ]]; then
  echo "DEVICE_ADVISOR_PASSWORD environment variable must be set!"
  return 1;
fi

set -e

DAEMONIZE=true

THING_NAME="mqttrust"
SUITE_ID="1gaev57dq6i5"
export RUST_LOG=debug 

THING_ARN="arn:aws:iot:eu-west-1:411974994697:thing/$THING_NAME"
SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"


export AWS_HOSTNAME=$(aws iotdeviceadvisor get-endpoint --thing-arn $THING_ARN --output text --query endpoint)

cargo build --features=log --example aws_device_advisor --release

SUITE_RUN_ID=$(aws iotdeviceadvisor start-suite-run --suite-definition-id $SUITE_ID --suite-run-configuration "primaryDevice={thingArn=$THING_ARN},parallelRun=true" --output text --query suiteRunId)
if $DAEMONIZE; then
    nohup ./target/release/examples/aws_device_advisor > device_advisor_integration.log &
    PID=$!
else
    echo "You can now run 'DEVICE_ADVISOR_PASSWORD=$DEVICE_ADVISOR_PASSWORD AWS_HOSTNAME=$AWS_HOSTNAME ./target/release/examples/aws_device_advisor' in a seperate terminal"
fi

always() {
    kill $PID || true
    cat device_advisor_integration.log
}

on_failure() {
    if $DAEMONIZE; then
        always || true
    fi
    aws iotdeviceadvisor stop-suite-run --suite-definition-id $SUITE_ID --suite-run-id $SUITE_RUN_ID
}

trap "on_failure" ERR INT

$SCRIPT_DIR/da_monitor.sh $SUITE_ID $SUITE_RUN_ID $PID

always

