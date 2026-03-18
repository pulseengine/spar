/** @module Interface pulseengine:rivet/renderer@0.1.0 **/
export function render(root: string, highlight: Array<string>): string;
export type RenderError = RenderErrorParseError | RenderErrorNoRoot | RenderErrorLayoutError;
export interface RenderErrorParseError {
  tag: 'parse-error',
  val: string,
}
export interface RenderErrorNoRoot {
  tag: 'no-root',
  val: string,
}
export interface RenderErrorLayoutError {
  tag: 'layout-error',
  val: string,
}
