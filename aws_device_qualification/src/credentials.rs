use std::env;

pub fn identity() -> (Vec<u8>, Vec<u8>) {
    let pw = env::var("DEVICE_ADVISOR_PASSWORD").unwrap();
    let pfx = include_bytes!("secrets/identity.pfx");
    let pkcs = openssl::pkcs12::Pkcs12::from_der(pfx).unwrap();
    let inner = pkcs.parse2(&pw).unwrap();
    (
        inner.pkey.unwrap().private_key_to_der().unwrap(),
        inner.cert.unwrap().to_der().unwrap(),
    )
}

pub fn root_ca() -> Vec<u8> {
    let pem = include_bytes!("secrets/root-ca.pem");
    openssl::x509::X509::from_pem(pem)
        .unwrap()
        .to_der()
        .unwrap()
}

pub fn hostname() -> String {
    env::var("AWS_HOSTNAME").unwrap()
}
