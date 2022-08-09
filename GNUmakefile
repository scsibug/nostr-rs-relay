-: build
	mkdir -p data
build:
	docker build -t nostr-rs-relay .
run:
	docker run -it -p 7000:8080 \
		--mount src=$(PWD)/config.toml,target=/usr/src/app/config.toml,type=bind \
		--mount src=$(PWD)/data,target=/usr/src/app/db,type=bind \
		nostr-rs-relay
noscl:
	go install github.com/fiatjaf/noscl@latest
all: - init build run noscl
init:
	test go && brew install golang || apt install golang-go || true
