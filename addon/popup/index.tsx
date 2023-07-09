import React, { useEffect, useState } from 'react';
import ReactDOM from 'react-dom';
import { PortRpc } from '../rpc';
import { Address } from '../packet';
import { generateId } from '../inpage';
import { Alert, Button, Card, Checkbox, ConfigProvider, Dropdown, Layout, List, Space, Typography, theme } from 'antd';
import { Content, Header } from 'antd/es/layout/layout';
import 'antd/dist/reset.css';
const { Text, Title } = Typography;

console.log('Hello from popup!');
const rpc = new PortRpc(Address.Popup);

type RequestedDevice = {
	id: string,
	vid: number,
	pid: number,
	product_name: string,
	serial: string,
	persistent: boolean,
};
function Device(dev: RequestedDevice & { choosen: boolean, onChange: (v: boolean) => void }) {
	return <List.Item style={{ width: '100%' }}>
		<List.Item.Meta style={{ width: '100%' }}
			title={
				<Checkbox checked={dev.choosen} onChange={e => dev.onChange(e.target.checked)}>
					<Space>
						<Text>{dev.product_name}</Text><Text type="secondary">{dev.serial}</Text>
					</Space>
				</Checkbox>
			}
			description={
				<Text><b>vid/pid:</b> {dev.vid.toString(16).padStart(4, '0')}/{dev.pid.toString(16).padStart(4, '0')} {dev.persistent ? null : <b>Can't remember!</b>}</Text>
			}

		/>
	</List.Item>
}

type RequestAccess = {
	synteticRequestId?: string,
	result?: (r: RequestAccessResult) => void;
	devices: RequestedDevice[];
};
type RequestAccessResult = {
	approved: string[],
};

// TODO: Expiration
function Request(r: RequestAccess & { onComplete: (devs: string[], duration: number | null) => void }) {
	const [chosen, setChosen] = useState(new Set<string>());
	const [ikwid, setIkwid] = useState(false);

	return <Card title="Device access request" style={{ width: '100%' }}>
		<Alert message='Make sure you trust this website; HID access may allow site to access your input devices, including muting/unmuting of the microphone' type="warning" showIcon />

		<Space direction='vertical' style={{ width: '100%' }}>
			{r.devices.length === 0 ? <Alert message="No supported devices found" type="error" showIcon /> : <List bordered>

				{r.devices.map(dev => <Device {...dev} key={dev.id} choosen={chosen.has(dev.id)} onChange={(set) => {
					let newChosen = new Set(chosen);
					if (set) newChosen.add(dev.id);
					else newChosen.delete(dev.id);
					setChosen(newChosen)
				}} />)}
			</List>}


			<Space>
				<Checkbox checked={ikwid} onChange={e => setIkwid(e.target.checked)}>I know what I'm doing</Checkbox>
			</Space>
			<Space.Compact block>
				<AcceptBtn blocked={!ikwid || chosen.size === 0} onClick={(v: number | null) => r.onComplete(Array.from(chosen), v)} />
				<Button type="default" onClick={() => r.onComplete([], 0)}>Reject</Button>
			</Space.Compact>
		</Space>
	</Card>
}
function AcceptBtn(r: { blocked: boolean, onClick: (v: number | null) => void }) {
	const [time, setTime] = useState(5);
	let timeout: any;
	useEffect(() => {
		timeout = setInterval(() => setTime(t => t - 1), 1000);
		return () => clearInterval(timeout);
	}, [r.blocked]);
	return <>
		{r.blocked ? <Button type="primary" danger disabled>
			Allow
		</Button> : time > 0 ? <Button type="primary" danger disabled>
			Wait {time}s...
		</Button> : <Dropdown.Button type="primary" danger menu={{
			items: [
				{
					key: '' + (1 * 1000 * 60 * 60),
					label: 'Allow for 1 hour'
				},
				{
					key: '' + (10 * 1000 * 60 * 60),
					label: 'Allow for 10 hours'
				},
				{
					key: 'null',
					label: 'Always allow',
				},
			],
			onClick: (v) => r.onClick(JSON.parse(v.key))
		}} onClick={() => r.onClick(0)}>
			Allow this time
		</Dropdown.Button>}
	</>
}

const useThemeDetector = () => {
	const getCurrentTheme = () => window.matchMedia("(prefers-color-scheme: dark)").matches;
	const [isDarkTheme, setIsDarkTheme] = useState(getCurrentTheme());
	const mqListener = ((e: any) => {
		setIsDarkTheme(e.matches);
	});

	useEffect(() => {
		const darkThemeMq = window.matchMedia("(prefers-color-scheme: dark)");
		darkThemeMq.addListener(mqListener);
		return () => darkThemeMq.removeListener(mqListener);
	}, []);
	return isDarkTheme;
}
function Root() {
	const [list, setList] = useState<RequestAccess[]>([]);
	const isDark = useThemeDetector();

	useEffect(() => {
		rpc.addRequestListener<RequestAccess, RequestAccessResult>('RequestAccess', (sender, req) => {
			if (sender !== Address.Native) return Promise.resolve({ approved: [] });

			return new Promise(res => {
				req.synteticRequestId = generateId();
				req.result = res;
				setList(list => [...list, req])
			});
		})

		const backgroundPort = browser.runtime.connect({ name: 'popup' });
		rpc.addDirect(Address.Background, backgroundPort, 50);
		console.log('opened rpc connection');
	}, [])

	return <ConfigProvider theme={{ algorithm: isDark ? theme.darkAlgorithm : theme.defaultAlgorithm }}>
		<Layout style={{ width: '100%', height: '100%' }}>
			<Header>
				Firefox WebHID
			</Header>
			<Content>
				{list.map(entry => <Request key={entry.synteticRequestId} devices={entry.devices} onComplete={(devs) => {
					entry.result!({ approved: devs });
					setList(list => {
						const newList = list.filter(e => e.synteticRequestId !== entry.synteticRequestId);
						if (newList.length === 0) window.close();
						return newList;
					});
				}} />
				)}
			</Content>
		</Layout>
	</ConfigProvider>
}
ReactDOM.render(
	<React.StrictMode>
		<Root />
	</React.StrictMode >,
	document.body
)
