# Local embeddings image

The provider runtime template **Local embeddings** builds this recipe on its host. It runs
[llama.cpp](https://github.com/ggml-org/llama.cpp)'s server (release b11514) with
[EmbeddingGemma 2](https://huggingface.co/ggml-org/embeddinggemma-2-GGUF) in its Q8_0 quantization (309,855,456
bytes, Apache-2.0, more than a hundred languages), answering the Embeddings format (`POST /v1/embeddings`) on the
runtime's own loopback. Orch's search by meaning sends batches of texts there through the provider's connector,
as it would to any API endpoint of that format; nothing else reaches it.

The [Dockerfile](Dockerfile) pins the Debian base by digest, fetches the server's release archive for the host's
architecture (x86_64 or arm64) and the model file at a fixed revision, and checks all three by their SHA-256 before
anything uses them; the server needs the OpenMP runtime and OpenSSL 3 from Debian's signed packages. Neither the
server nor the model is part of Orch's repository or release: the host that builds the image fetches them. It builds
with the classic builder and with BuildKit, and needs no context or build arguments. On x86_64 Docker's image
inspection gives 331 MB (331,453,181 bytes), most of it the model.

The template starts the server after the unit starts (`/opt/llama/serve`, a command after start) with llama.cpp's
own variables (`LLAMA_ARG_*`): the model, its name `embeddinggemma-2`, mean pooling, embeddings only, four threads,
a context of 2,048 tokens and port 8090 on 127.0.0.1; the command waits until the server answers its health check,
for at most `EMBEDDINGS_START_SECS` seconds. Its limits are the template's ordinary defaults (four CPUs, 2 GiB,
512 processes, a read-only root, no new privileges, all capabilities dropped), changed like any unit's. The server
answers with the model's own 768 numbers whatever length is asked; Orch keeps the first 256 and makes them unit
length again, as the model's card says it may be used.

```sh
docker build --tag orch-embeddings:local images/orch-embeddings
```

Measured on x86_64 in a container of four CPUs and 2 GiB, as the template runs it: the server answers three seconds
after it starts; it takes about 0.65 GiB once it has worked; it makes 176 to 259 tokens of vectors a second for
batches of sixteen texts, depending on what else the machine runs; and texts of the same meaning in English and
Chinese come out nearer each other than texts of other meanings.
