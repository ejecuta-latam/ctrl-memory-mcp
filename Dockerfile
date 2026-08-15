# Imagen runtime liviana: ubuntu 24.04 (glibc 2.39 = la del runner de CI).
#
# El binario se compila FUERA de la imagen (cargo build --release en el runner
# ubuntu-latest, glibc 2.39) y se copia desde target/. memory-mcp NO se
# compila musl: la dependencia ort (ONNX Runtime) es glibc-only. Por eso la
# base es ubuntu 24.04 (misma glibc que el runner), como agents-registry.
FROM ubuntu:24.04

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY target/release/memory-mcp /app/memory-mcp

EXPOSE 8737
ENTRYPOINT ["/app/memory-mcp"]
