export enum Address {
	Native = 'Native',
	Background = 'Background',
	Popup = 'Popup',
	Content = 'Content',
	Injected = 'Injected',
};

export type ResponsePacketHeader = {
	rid: string,
	request_origin: Address,
	error?: string,
};
export type RequestPacketHeader = {
	sender: Address,
	receiver: Address,
	request: string,
	response?: {
		rid: string,
	},
};
export type PacketHeader = RequestPacketHeader | ResponsePacketHeader;
