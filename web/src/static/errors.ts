// A layer's data could not be loaded and there is nowhere else to run the query (browser-only deployment): the data host did not answer, a
// file was malformed, the browser ran out of memory. It is NOT "server required": the analysis is possible, this layer's data is not reachable.
// The UI says so plainly and every other layer keeps working. No DOM: shared by the placement logic and the UI.
export class LayerUnavailable extends Error {
  readonly reason: string;
  constructor(reason: string) {
    super(`layer data could not be loaded: ${reason}`);
    this.name = "LayerUnavailable";
    this.reason = reason;
  }
}
