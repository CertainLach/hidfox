native/native.json: native/native.jsonnet
	jrsonnet -e "function(path) std.manifestJsonEx((import '$<')(path = path),'', newline='',key_val_sep=':')" -S --tla-str=path="$(PWD)/target/debug/webhid-firefox" > $@
addon-dist/manifest.json: addon/manifest.jsonnet
	jrsonnet -e "std.manifestJsonEx(import '$<','', newline='',key_val_sep=':')" -S > $@

addon-dist/icons: addon/icons/icon256.png
	mkdir $@
	cp $< $@/icon256.png
	convert $< -resize 64x64 $@/icon64.png
	convert $< -resize 48x48 $@/icon48.png
	convert $< -resize 32x32 $@/icon32.png
	convert $< -resize 16x16 $@/icon16.png

.PHONY: addon-dist
addon-dist: addon-dist/manifest.json addon-dist/icons
	cd addon && yarn webpack
	#rm addon-dist/popup.js.LICENSE.txt # Only MIT here, reduce published extension size.

.PHONY: native
native: native/native.json
	cargo build

HOSTS=$(HOME)/.mozilla/native-messaging-hosts
install: addon-dist native
	mkdir -p $(HOSTS)
	cp native/native.json $(HOSTS)/hidfox.json

test: install
	cd addon-dist && web-ext run --browser-console --devtools --pref=extensions.openPopupWithoutUserGesture.enabled=true -u https://polkadot.dotapps.io/
