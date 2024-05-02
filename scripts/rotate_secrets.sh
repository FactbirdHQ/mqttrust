#!/usr/bin/env bash

if [[ -z "$DEVICE_ADVISOR_PASSWORD" ]]; then
  echo "DEVICE_ADVISOR_PASSWORD environment variable must be set!"
  return 1;
fi

SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
SECRETS_DIR="$SCRIPT_DIR/../mqttrust_core/examples/secrets"
THING_NAME="mqttrust"

CERT_PATH="$SECRETS_DIR/cert.pem"
PRIV_KEY_PATH="$SECRETS_DIR/priv.key.pem"

CERT_ARN=$(aws iot create-keys-and-certificate --set-as-active --certificate-pem-outfile $CERT_PATH --private-key-outfile $PRIV_KEY_PATH | jq -r .certificateArn);
aws iot attach-thing-principal --thing-name $THING_NAME --principal $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Connect --target $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Input --target $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Output --target $CERT_ARN > /dev/null 2>&1

rm $SECRETS_DIR/identity.pfx

# Generate new identity.pfx
openssl pkcs12 -export -passout pass:"$DEVICE_ADVISOR_PASSWORD" -out $SECRETS_DIR/identity.pfx -inkey $PRIV_KEY_PATH -in $CERT_PATH

rm $CERT_PATH
rm $PRIV_KEY_PATH