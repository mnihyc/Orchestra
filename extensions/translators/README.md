# Model translators

The shipped responses-messages extension implements Responses to Messages and Messages to Responses through the neutral
model contract. Its fidelity declarations also supply provider formats, binding choices and generation route inspection.
Additional external translators use the same contract, hosted by the plugin runtime beside the credential.

The outgoing wire protocol presents the key and observes upstream usage before conversion. Direct endpoint matches take
priority. A translated pairing is chosen through normal binding and held to the inherited translation policy.
