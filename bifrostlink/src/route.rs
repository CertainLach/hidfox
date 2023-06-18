use std::{
	collections::{hash_map::Entry, HashMap, HashSet},
	hash::Hash,
};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender as Sender;

use crate::{event::RootEvent, AddressT};

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub enum Via<Address> {
	Address(Address),
	Direct,
}
impl<Address> Via<Address>
where
	Address: Clone,
{
	fn as_address(&self) -> Option<Address> {
		match self {
			Via::Address(a) => Some(a.clone()),
			Via::Direct => None,
		}
	}
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Serialize, Deserialize, Copy, Debug)]
pub struct Rtt(pub u32);

#[derive(PartialEq, Clone, Debug)]
pub struct MinRtt<Address> {
	pub via: Via<Address>,
	pub rtt: Rtt,
	pub second_best: Option<Rtt>,
}

#[derive(Debug)]
pub struct MinRttUpdated<Address> {
	pub for_address: Address,
	pub rtt: MinRtt<Address>,
	pub first_changed: bool,
	pub second_changed: bool,
}

#[derive(Debug)]
pub struct ViaListSeconded<Address> {
	pub for_connection: Address,
	pub initial_via: Via<Address>,
	pub added_via: Via<Address>,
	pub rtt: Rtt,
}

#[derive(Debug)]
pub struct ViaListUnseconded<Address> {
	pub for_connection: Address,
	pub only_via: Via<Address>,
}

#[derive(Debug)]
pub struct ConnectionAdded<Address> {
	pub to: Address,
	pub via: Via<Address>,
	pub rtt: Rtt,
}
#[derive(Debug)]
pub struct ConnectionRemoved<Address> {
	pub to: Address,
	pub via: Via<Address>,
}

struct AddressData<Address> {
	// address: Address,
	via: HashMap<Via<Address>, Rtt>,
	min_rtt: MinRtt<Address>,
}
impl<Address> AddressData<Address>
where
	Address: Clone + PartialEq,
{
	fn update_min_rtt(&mut self, for_address: Address, sender: &mut Sender<RootEvent<Address>>) {
		let (via, rtt) = self
			.via
			.iter()
			.min_by_key(|(_, rtt)| **rtt)
			.expect("updated address with no routes");
		let second_best = self
			.via
			.iter()
			.filter(|(second, _)| second != &via)
			.map(|(_, rtt)| rtt)
			.min();

		let old = &self.min_rtt;
		let new = MinRtt {
			via: via.clone(),
			rtt: rtt.clone(),
			second_best: second_best.cloned(),
		};

		if old == &new {
			return;
		}

		let only_first_updated = &old.rtt != rtt;
		let only_second_updated = old.second_best.as_ref() != second_best;
		let min_via_updated = &old.via != via;

		if let Err(_) = sender.send(
			MinRttUpdated {
				for_address,
				rtt: new.clone(),
				first_changed: only_first_updated || min_via_updated,
				second_changed: only_second_updated || min_via_updated,
			}
			.into(),
		) {
			eprintln!("no handlers for min rtt update")
		}

		self.min_rtt = new;
	}
}

#[derive(derivative::Derivative)]
#[derivative(Default(bound = ""))]
struct InverseRouteSet<Address> {
	vias: HashMap<Via<Address>, HashSet<Address>>,
}
impl<Address> InverseRouteSet<Address>
where
	Address: AddressT,
{
	fn inc(&mut self, via: Via<Address>, to: Address) {
		let routes = self.vias.entry(via).or_default();
		assert!(routes.insert(to), "inverse imbalance (double inc)");
	}
	fn dec(&mut self, via: Via<Address>, to: Address) {
		let routes = self
			.vias
			.get_mut(&via)
			.expect("inverse imbalance (unknown dec)");
		assert!(routes.remove(&to), "inverse imbalance (double dec route)");
		if routes.is_empty() {
			self.vias.remove(&via);
		}
	}
	fn forwarded(&self, via: Via<Address>) -> Option<impl Iterator<Item = Address> + '_> {
		let routes = self.vias.get(&via)?;
		Some(routes.into_iter().cloned())
	}
}

pub struct RouteSet<Address> {
	routes: HashMap<Address, AddressData<Address>>,
	inverse: InverseRouteSet<Address>,
	event: Sender<RootEvent<Address>>,
}

impl<Address> RouteSet<Address>
where
	Address: AddressT,
{
	pub fn new(tx: Sender<RootEvent<Address>>) -> Self {
		Self {
			routes: Default::default(),
			inverse: Default::default(),
			event: tx,
		}
	}
	pub fn inc(&mut self, address: Address, via: Via<Address>, rtt: Rtt) {
		match self.routes.entry(address.clone()) {
			Entry::Occupied(mut v) => {
				let data = v.get_mut();
				let seconded_initial = (data.via.len() == 1).then(|| {
					let (via, rtt) = data.via.iter().next().unwrap().clone();
					(via.clone(), rtt.clone())
				});
				{
					let Entry::Vacant(via) = data.via.entry(via.clone()) else {
                        eprintln!("added duplicate connection: {address:?} via {via:?}");
                        return;
                    };
					via.insert(rtt.clone());
				}
				if let Some((initial_via, initial_rtt)) = seconded_initial {
					if let Err(_) = self.event.send(
						ViaListSeconded {
							for_connection: address.clone(),
							initial_via: initial_via.clone(),
							added_via: via.clone(),
							rtt: rtt.min(initial_rtt.clone()),
						}
						.into(),
					) {
						eprintln!("no listener for ViaListSeconded")
					}
				}
				let via = v.key().clone();
				v.get_mut().update_min_rtt(via, &mut self.event);
			}
			Entry::Vacant(v) => {
				v.insert(AddressData {
					// address: address.clone(),
					via: [(via.clone(), rtt)].into_iter().collect(),
					min_rtt: MinRtt {
						via: via.clone(),
						rtt,
						second_best: None,
					},
				});
				if let Err(_) = self.event.send(
					ConnectionAdded {
						to: address.clone(),
						via: via.clone(),
						rtt,
					}
					.into(),
				) {
					eprintln!("no listener for ConnectionAdded")
				}
			}
		}
		self.inverse.inc(via, address)
	}
	pub fn dec(&mut self, address: Address, via: Via<Address>) {
		let Some(data) = self.routes.get_mut(&address) else {
            eprintln!("removed unknown connection: {address:?} via {via:?} (there is no routes to the specified address)");
            return;
        };
		if data.via.remove(&via).is_none() {
			eprintln!("removed unknown connection: {address:?} via {via:?}");
			return;
		}
		if data.via.is_empty() {
			self.routes.remove(&address);
			if let Err(_) = self.event.send(
				ConnectionRemoved {
					to: address.clone(),
					via: via.clone(),
				}
				.into(),
			) {
				eprintln!("no listener for ConnectionRemoved");
			}
		} else {
			if data.via.len() == 1 {
				let only_via = data.via.keys().next().expect("len == 1").clone();
				if let Err(_) = self.event.send(
					ViaListUnseconded {
						for_connection: address.clone(),
						only_via,
					}
					.into(),
				) {
					eprintln!("no listener for ConnectionRemoved");
				}
			}
			data.update_min_rtt(address.clone(), &mut self.event);
		}
		self.inverse.dec(via, address)
	}
	pub fn update(&mut self, address: Address, via: Via<Address>, rtt: Rtt) {
		let Some(data) = self.routes.get_mut(&address) else {
            eprintln!("updated rtt for unknown connection");
            return;
        };
		let Some(viartt) = data.via.get_mut(&via) else {
            eprintln!("updated rtt for unknown connection");
            return;
        };
		*viartt = rtt;
		data.update_min_rtt(address, &mut self.event)
	}
	pub fn has(&self, address: Address) -> bool {
		self.routes.contains_key(&address)
	}
	pub fn list(&self) -> impl Iterator<Item = (Address, MinRtt<Address>)> + '_ {
		self.routes
			.iter()
			.map(|(a, d)| (a.clone(), d.min_rtt.clone()))
	}

	pub fn may_be_forwarder_for(&self, forwarder: Via<Address>, sender: Address) -> bool {
		if forwarder.as_address().as_ref() == Some(&sender) {
			return true;
		}
		let Some(connections) = self.routes.get(&sender) else {
            // No connection
            return false;
        };
		connections.via.contains_key(&forwarder)
	}
	pub fn forwarder_for(
		&self,
		address: Address,
		blacklist: &HashSet<Via<Address>>,
	) -> Option<Via<Address>> {
		let connections = self.routes.get(&address)?;
		// Has direct connection
		if connections.via.contains_key(&Via::Direct) {
			return Some(Via::Direct);
		}

		// Best possible
		connections
			.via
			.iter()
			.filter(|(via, _)| !blacklist.contains(&via))
			.min_by_key(|(_, rtt)| **rtt)
			.map(|(via, _)| via.clone())
	}

	pub fn on_add_direct_connection(&mut self, address: Address, rtt: Rtt) {
		self.inc(address, Via::Direct, rtt);
	}
	pub fn on_remove_direct_connection(&mut self, address: Address) {
		self.dec(address, Via::Direct);
	}
}
