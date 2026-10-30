# Additional certificate authorities

Put each authority's public certificate in a separate PEM file here, make it readable by the services, then run:

```bash
openssl rehash ./config/ca-certificates
```

Restart the API, gateway, worker and node. They trust these authorities in addition to the release's public root bundle. The services mount this directory read-only; private keys stay with the issuer.

See [Certificates](../../../docs/operator/edge.md#certificates) for public certificates, internal authorities and the pinned gateway path.
