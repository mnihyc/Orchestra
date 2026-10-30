# orch-provider-contract

Provider source contracts: facts, models and credential presentation.

The interfaces through which a provider's facts (with the resets they list), its models and the presentation of its key are read: `FactsSource`, `ModelsSource` and `CredentialPresenter`, implemented alike by the built-in readers, by a program registered beside the credential (plugin kind `provider`) and by an address reached over HTTP or HTTPS. It holds what a source declares (`SourceDescriptor`), what every call is given (`SourceAsk`, `PresentAsk`) and answers, the errors every binding shares, what a provider's managers chose for each element as it is kept (`SourceChoice`, `ProviderSources`) and as the server tells the keeping machine (`SourceSpec`), the address envelope with its authorization, and the method names, deadlines and bounds. The types of a provider's facts with their four states and their normalization (`facts`), and what a model can be run with (`models`), are written here once and re-exported by the agent contract and the API's, so this contract depends on neither. Its unit tests read and write each shape as a program, an address and the link write it, and check descriptors, choices, address replies and bounds.

| Module | Planned responsibility |
|--------|------------------------|
| [address](src/address.rs) | The address binding: a program's methods and JSON over HTTP; its envelope, its authorization, and how its reply is read. |
| [builtins](src/builtins.rs) | The built-in sources of a key's facts a program carries: each one's name, its declaration, the addresses it is the default for, and how it is made with the keeping machine's transport. |
| [facts](src/facts.rs) | What a provider's sources report of it: the plan, how full its windows are, what is left to spend and what was spent, the resets it offers, and who its credential belongs to. |
| [forecast](src/forecast.rs) | A shared projection of measured plan usage or a cap's counted period use to its reset. |
| [failure](src/failure.rs) | Shared classification of model failures from structured evidence, status and vendor words. |
| [models](src/models.rs) | What a model can be run with at a provider, and a model as a list of models gives it. |
| [choice](src/choice.rs) | What a provider's managers chose for each of its elements, as it is kept and as the server tells the machine that keeps the credential for one call. |
| [calls](src/calls.rs) | Inherited recovery rules, ordered steps and restartable progress for every model-call purpose. |
| [limits](src/limits.rs) | The bounds every answer of a source is held to, and how long a learned answer is kept. |
| [members](src/members.rs) | The members of a provider's facts, the four states each is in, and which members a provider's sources declare. |
| [source](src/source.rs) | The interfaces a provider's facts, models and key presentation are read through, what a source declares, what it is given and what it answers. |
| [wire](src/wire.rs) | The methods of a provider source, the version of the contract they are written in, their parameters, and how long each may take. |
