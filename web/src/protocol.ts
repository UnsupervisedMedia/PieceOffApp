// The wire protocol as the web client sees it: the generated serde types
// (protocol.gen.ts, from pieceoff-core) plus the version handshake check.
export * from './protocol.gen';

export type ProtocolCheck = 'ok' | 'update-client' | 'update-server';

/**
 * Compare the server's protocol version (from `Welcome.snapshot` or the
 * room listing) with the one this client was built for. Versions must match
 * exactly; the result says which side is behind.
 */
export function checkServerProtocol(server: number, client: number): ProtocolCheck {
  if (server === client) return 'ok';
  return server > client ? 'update-client' : 'update-server';
}
