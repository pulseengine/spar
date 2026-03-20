/** @module Interface pulseengine:rivet/types@0.1.0 **/
export interface Link {
  linkType: string,
  target: string,
}
export type FieldValue = FieldValueText | FieldValueNumber | FieldValueBoolean | FieldValueTextList;
export interface FieldValueText {
  tag: 'text',
  val: string,
}
export interface FieldValueNumber {
  tag: 'number',
  val: number,
}
export interface FieldValueBoolean {
  tag: 'boolean',
  val: boolean,
}
export interface FieldValueTextList {
  tag: 'text-list',
  val: Array<string>,
}
export interface FieldEntry {
  key: string,
  value: FieldValue,
}
export interface Artifact {
  id: string,
  artifactType: string,
  title: string,
  description?: string,
  status?: string,
  tags: Array<string>,
  links: Array<Link>,
  fields: Array<FieldEntry>,
}
export interface ConfigEntry {
  key: string,
  value: string,
}
export interface AdapterConfig {
  entries: Array<ConfigEntry>,
}
export type AdapterError = AdapterErrorParseError | AdapterErrorValidationError | AdapterErrorIoError | AdapterErrorNotSupported;
export interface AdapterErrorParseError {
  tag: 'parse-error',
  val: string,
}
export interface AdapterErrorValidationError {
  tag: 'validation-error',
  val: string,
}
export interface AdapterErrorIoError {
  tag: 'io-error',
  val: string,
}
export interface AdapterErrorNotSupported {
  tag: 'not-supported',
  val: string,
}
