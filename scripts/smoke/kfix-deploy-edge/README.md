# Readiness through the web edge

Run `bash scripts/smoke/kfix-deploy-edge/run.sh` with Docker and the smoke helpers' release inputs.
The scenario checks this checkout's updater against the verified release programs, using real nginx,
API, gateway and PostgreSQL services. Candidates with a missing page and a missing nginx process must
return the prior payloads and database. A candidate-only database write must disappear and the prior
administrator must sign in again. A good update must serve its release through the operator URL.
