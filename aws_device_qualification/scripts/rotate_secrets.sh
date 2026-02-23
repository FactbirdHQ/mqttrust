#!/usr/bin/env bash

if [[ -z "$DEVICE_ADVISOR_PASSWORD" ]]; then
  echo "DEVICE_ADVISOR_PASSWORD environment variable must be set!"
  return 1;
fi

SCRIPT_DIR="$(dirname "$(readlink -f "$0")")"
SECRETS_DIR="$SCRIPT_DIR/../secrets"
CLIENT_CONFIG="$SECRETS_DIR/client.config"
THING_NAME="mqttrust"

CERT_PATH="$SECRETS_DIR/cert.pem"
PRIV_KEY_PATH="$SECRETS_DIR/priv.key.pem"
CSR_PATH="$SECRETS_DIR/client.csr.pem"

openssl ecparam -name prime256v1 -genkey -noout -out $PRIV_KEY_PATH
openssl req -new -sha256 -config $CLIENT_CONFIG -key $PRIV_KEY_PATH -out $CSR_PATH

CERT_ARN=$(aws iot create-certificate-from-csr --certificate-signing-request file://$CSR_PATH --set-as-active --certificate-pem-outfile $CERT_PATH | jq -r .certificateArn);
for OLD_CERT in $(aws iot list-thing-principals --thing-name $THING_NAME | jq -r '.principals[]' | xargs); do
  CERT_ID=$(echo $OLD_CERT | cut -d "/" -f 2)
  aws iot detach-thing-principal --thing-name $THING_NAME --principal $OLD_CERT
  aws iot update-certificate --new-status INACTIVE --certificate-id $CERT_ID
  aws iot delete-certificate --certificate-id $CERT_ID --force-delete
done
aws iot attach-thing-principal --thing-name $THING_NAME --principal $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Connect --target $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Input --target $CERT_ARN > /dev/null 2>&1
aws iot attach-policy --policy-name Output --target $CERT_ARN > /dev/null 2>&1

rm -f $SECRETS_DIR/identity.pfx

# Generate new identity.pfx
openssl pkcs12 -export -passout pass:"$DEVICE_ADVISOR_PASSWORD" -out $SECRETS_DIR/identity.pfx -inkey $PRIV_KEY_PATH -in $CERT_PATH

rm $CERT_PATH
rm $PRIV_KEY_PATH
rm $CSR_PATH