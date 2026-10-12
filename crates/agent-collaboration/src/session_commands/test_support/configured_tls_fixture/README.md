# Public localhost TLS fixture

These certificate and key bytes are deliberately public test data, used only by the local configured-reader TLS tests. They carry no user or production identity and are never installed in system trust. Tests load the certificate as a trust anchor only through an isolated child process's SSL_CERT_FILE.

Generated with LibreSSL3.3.6: a self-signed RSA2048 localhost server certificate, CA:FALSE, digitalSignature/keyEncipherment, serverAuth and DNS:localhost. The fixed fixture avoids a runtime dependency on OpenSSL or a new certificate-generation package. The certificate expires in2126. The private-key DER is public fixture material and must never secure a deployed service.
