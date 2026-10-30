# orch-pki

Certificate authority and machine identity mechanisms.

Implemented. The package holds the system's own authority (made once, its
key kept in a file only its owner reads), the keys and requests of a machine,
the certificates the authority issues for machines and services, and the
verifiers of both trust paths: the pinned path under the authority, and the
public path through an edge, with public and machine-installed authorities, where a machine proves itself by signing what the
gateway sent. It also mints and redeems enrollment tokens that carry the pin,
renews certificates (an expired machine certificate within its grace as well),
seals a secret to one machine's key, and signs the tickets that allow a tunnel.
It is tested with real keys and certificates, TLS handshakes between the
configurations it builds, and a clock that moves when told.

| Module | Planned responsibility |
|--------|------------------------|
| [ca](src/ca.rs) | CA initialization and custody. |
| [certificates](src/certificates.rs) | Local keys, CSRs and certificate issuance. |
| [verification](src/verification.rs) | Pinned and public-trust verification. |
| [enrollment](src/enrollment.rs) | Enrollment token and trust-pin mechanisms. |
| [renewal](src/renewal.rs) | Key rotation and renewal mechanisms. |
| [sealing](src/sealing.rs) | HPKE machine-to-machine secret sealing. |
| [service_renew](src/service_renew.rs) | A service's own certificate kept current while it runs: shown live to every new handshake, renewed once two thirds of its life have passed, adopted only for the new key and the same holder. |
| [tickets](src/tickets.rs) | Signed tunnel ticket mechanisms. |
| [port_assertions](src/port_assertions.rs) | Port caller assertions: the short-lived signed statement of who a port's caller is. |
