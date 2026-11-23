/**
 * The client container: what a form injects once the connection exists. The connection's `Client` is the class
 * token, its state is `ConnectionState` (an action), and each server controller is `remoteToken(Controller)`, the
 * controller's `Remote<C>` over that connection. All of them live in `kernel.container`.
 */
import { Container, createToken, type Token } from "@coldmind/inject";
import { remoteOf, stateAction, Client, type ClientState, type Remote } from "@coldmind/net.client";
import type { Action } from "@coldmind/wire";
import { kernel } from "@coldmind/vcl.core";

type Controller = abstract new (...args: never[]) => object;

/** The connection state ("up", "down", "closed") as an action. */
export const ConnectionState = createToken<Action<ClientState>>("ConnectionState");

const remoteTokens = new Map<Controller, Token<unknown>>();

/** The token a form injects for the remote side of `ctor`, a class decorated with `@Controller`. */
export function remoteToken<C extends object>(ctor: abstract new (...args: never[]) => C): Token<Remote<C>> {
  let token = remoteTokens.get(ctor);
  if (!token) {
    token = createToken<Remote<C>>(`remote:${ctor.name}`);
    remoteTokens.set(ctor, token);
  }
  return token as Token<Remote<C>>;
}

/**
 * Registers the connection and its controllers in `container`, so a form resolves them by token. Call it once,
 * with the connection, before any form is created.
 *
 * @param client the connection the application runs over.
 * @param controllers the server's controller classes (`@Server`'s `controllers`).
 * @param container defaults to the kernel's root container.
 */
export function provideClient(client: Client, controllers: readonly Controller[], container: Container = kernel.container): Client {
  container.registerInstance(Client, client);
  container.registerInstance(ConnectionState, stateAction(client));
  for (const ctor of controllers) container.registerInstance(remoteToken(ctor), remoteOf(client, ctor));
  return client;
}
