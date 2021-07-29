// This file is part of Substrate.

// Copyright (C) 2020-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Middleware for RPC requests.

use std::{sync::atomic::{AtomicUsize, Ordering::SeqCst}, time::Instant};

use jsonrpc_core::{
	Call, FutureOutput, FutureResponse, Id, Metadata, Middleware as RequestMiddleware,
	Output, Request, Response
};
use prometheus_endpoint::{
	Registry, CounterVec, PrometheusError,
	Opts, register, U64
};

use futures::{future::Either, Future};


/// Metrics for RPC middleware
#[derive(Debug, Clone)]
pub struct RpcMetrics {
	rpc_calls: Option<CounterVec<U64>>,
}

impl RpcMetrics {
	/// Create an instance of metrics
	pub fn new(metrics_registry: Option<&Registry>) -> Result<Self, PrometheusError> {
		Ok(Self {
			rpc_calls: metrics_registry.map(|r|
				register(
					CounterVec::new(
						Opts::new(
							"rpc_calls_total",
							"Number of rpc calls received",
						),
						&["protocol"]
					)?,
					r,
				)
			).transpose()?,
		})
	}
}

/// Middleware for RPC calls
pub struct RpcMiddleware {
	metrics: RpcMetrics,
	transport_label: String,
	counter: AtomicUsize,
}

impl RpcMiddleware {
	/// Create an instance of middleware.
	///
	/// - `metrics`: Will be used to report statistics.
	/// - `transport_label`: The label that is used when reporting the statistics.
	pub fn new(metrics: RpcMetrics, transport_label: &str) -> Self {
		RpcMiddleware {
			metrics,
			transport_label: String::from(transport_label),
			counter: AtomicUsize::new(0),
		}
	}
}

impl<M: Metadata> RequestMiddleware<M> for RpcMiddleware {
	type Future = FutureResponse;
	type CallFuture = FutureOutput;

	fn on_request<F, X>(&self, request: Request, meta: M, next: F) -> Either<FutureResponse, X>
	where
		F: Fn(Request, M) -> X + Send + Sync,
		X: Future<Item = Option<Response>, Error = ()> + Send + 'static,
	{
		if let Some(ref rpc_calls) = self.metrics.rpc_calls {
			rpc_calls.with_label_values(&[self.transport_label.as_str()]).inc();
		}

		Either::B(next(request, meta))
	}

	fn on_call<F, X>(&self, mut call: Call, meta: M, next: F) -> Either<Self::CallFuture, X>
	where
		F: Fn(Call, M) -> X + Send + Sync,
		X: Future<Item = Option<Output>, Error = ()> + Send + 'static,
	{
		let start = Instant::now();
		let mut method_name = String::new();
		let mut server_id = 0;
		if log::log_enabled!(target: "rpc-intercept", log::Level::Debug) {
			server_id = self.counter.fetch_add(1, SeqCst);
			let req_id = match call {
				Call::MethodCall(ref mut method) => {
					method_name = method.method.to_owned();
					method.id.clone()
				}
				Call::Notification(ref notif) => {
					method_name = notif.method.to_owned();
					Id::Null
				},
				Call::Invalid { ref mut id } => id.to_owned(),
			};
			if log::log_enabled!(target: "rpc-intercept", log::Level::Trace) {
				let len = serde_json::to_string(&call).unwrap().len();
				log::trace!(
					target: "rpc-intercept",
					"Request [id: {} - server_id: {} - method: {} - {} bytes]",
					id_to_string(&req_id),
					server_id,
					method_name,
					len
				);
			}
		}

		Either::A(Box::new(next(call, meta).map(move |response_opt| {
			if log::log_enabled!(target: "rpc-intercept", log::Level::Debug) {
				if let Some(ref response) = response_opt {
					let len = serde_json::to_string(response).unwrap().len();

					match response {
						Output::Success(success) => log::debug!(
							target: "rpc-intercept",
							"Response [id: {} - server_id: {} - method: {} - {} bytes - Processing took {} ms]: success",
							id_to_string(&success.id),
							server_id,
							method_name,
							len,
							start.elapsed().as_millis()
						),
						Output::Failure(failure) => log::error!(
							target: "rpc-intercept",
							"Response [id: {:?} - server_id: {} - method: {} - {} bytes - Processing took {} ms]: {}",
							id_to_string(&failure.id),
							server_id,
							method_name,
							len,
							start.elapsed().as_millis(),
							failure.error
						),
					}
				};
			}
			response_opt
		})))
	}
}

fn id_to_string(id: &Id) -> String {
	match id {
		Id::Num(id) => { id.to_string() },
		Id::Str(id) => { id.to_owned() },
		Id::Null => { "null".to_owned() }
	}
}
