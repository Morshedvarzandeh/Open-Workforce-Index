FROM rust:1.87-bookworm AS engine
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY ontology ./ontology
RUN cargo build --release --locked -p workforce-cli --bin owi

FROM python:3.13-slim-bookworm
WORKDIR /app
COPY hosting/requirements.txt ./hosting/requirements.txt
RUN pip install --no-cache-dir -r hosting/requirements.txt \
    && useradd --uid 10001 --create-home owi \
    && mkdir -p /data/owi && chown -R owi:owi /data
COPY --from=engine /src/target/release/owi /usr/local/bin/owi-engine
COPY tools ./tools
COPY examples ./examples
COPY hosting ./hosting
COPY LICENSE ./LICENSE
USER owi
ENV OWI_BINARY=/usr/local/bin/owi-engine OWI_HOSTED_HOME=/data/owi PYTHONPATH=/app/tools
EXPOSE 8080
CMD ["gunicorn", "--config", "hosting/gunicorn.conf.py", "owi_hosted:create_app()"]
