import type { RoomDO } from "./room";

export interface Env {
  ROOMS: DurableObjectNamespace<RoomDO>;
}
