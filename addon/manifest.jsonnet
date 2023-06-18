{
	manifest_version: 2,
	name: 'HidFox (Beta)',
	version: '1.2',
	icons: {
		[''+size]: 'icons/icon%d.png' % size,
		for size in [256, 64, 48, 32, 16]
	},

	description: 'WebHID shim for Firefox',
	content_scripts: [
		{
			// Disable on insecure domains
			matches: ['https://*/*'],
			js: ['content.js'],
			run_at: 'document_start',
		}
	],
	background: {
		scripts: ['background.js'],
		persistent: true,
		// type: 'module',
	},
	// page_action: {
	// 	browser_style: true,
	// 	default_title: 'WebHID Firefox',
	// 	default_popup: 'popup.html',
	// },
	web_accessible_resources: ['injected.js', 'popup.js'],
	permissions: [
		'nativeMessaging',
	],
	browser_specific_settings: {
		gecko: {
			id: 'webhid-firefox@delta.rocks',
		}
	}
}
