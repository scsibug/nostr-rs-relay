IMAGE=ghcr.io/kofj/nostr-rs-relay
PLATFORM="linux/amd64,linux/arm64"
GIVER=$(shell git describe --match 'v[0-9]*' --dirty='.m' --tags --always)

info:
	@echo version ${GIVER}

image:
	@echo build image version ${GIVER}
	@docker buildx build --platform ${PLATFORM} -t ${IMAGE}:${GIVER} .
