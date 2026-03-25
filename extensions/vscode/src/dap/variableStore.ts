import { Variable } from './protocol';

export type VariableStoreOptions = {
  pageSize?: number;
  maxStringPreview?: number;
  maxHexPreviewBytes?: number;
  maxSortedKeys?: number;
};

type ListHandle = { kind: 'list'; vars: Variable[] };
type ArrayHandle = { kind: 'array'; items: unknown[]; offset: number };
type ObjectHandle = { kind: 'object'; obj: Record<string, unknown>; keys: string[]; offset: number };
type StringFullHandle = { kind: 'string_full'; full: string };
type BytesDetailsHandle = { kind: 'bytes_details'; original: string; bytes: Uint8Array };

type HandlePayload = ListHandle | ArrayHandle | ObjectHandle | StringFullHandle | BytesDetailsHandle;

const DEFAULT_PAGE_SIZE = 100;
const DEFAULT_MAX_STRING_PREVIEW = 240;
const DEFAULT_MAX_HEX_PREVIEW_BYTES = 32;
const DEFAULT_MAX_SORTED_KEYS = 2000;

function looksLikeStrkeyAddress(value: string): boolean {
  return value.length === 56 && (value.startsWith('G') || value.startsWith('C'));
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function truncatePreview(value: string, maxChars: number): { preview: string; truncated: boolean } {
  if (maxChars <= 0 || value.length <= maxChars) {
    return { preview: value, truncated: false };
  }
  return { preview: value.slice(0, Math.max(0, maxChars - 1)) + '…', truncated: true };
}

function isTypedAnnotationObject(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const maybe = value as Record<string, unknown>;
  return typeof maybe.type === 'string' && 'value' in maybe;
}

function decodeBytesString(raw: string): Uint8Array | null {
  try {
    if (raw.startsWith('0x')) {
      const hex = raw.slice(2);
      if (hex.length % 2 !== 0 || /[^0-9a-fA-F]/.test(hex)) {
        return null;
      }
      return Uint8Array.from(Buffer.from(hex, 'hex'));
    }
    if (raw.startsWith('base64:')) {
      const b64 = raw.slice('base64:'.length);
      return Uint8Array.from(Buffer.from(b64, 'base64'));
    }
    return null;
  } catch {
    return null;
  }
}

function bytesToHex(bytes: Uint8Array): string {
  return `0x${Buffer.from(bytes).toString('hex')}`;
}

function bytesToBase64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('base64');
}

export class VariableStore {
  private handles = new Map<number, HandlePayload>();
  private nextHandle = 1;

  private pageSize: number;
  private maxStringPreview: number;
  private maxHexPreviewBytes: number;
  private maxSortedKeys: number;

  constructor(options: VariableStoreOptions = {}) {
    this.pageSize = options.pageSize ?? DEFAULT_PAGE_SIZE;
    this.maxStringPreview = options.maxStringPreview ?? DEFAULT_MAX_STRING_PREVIEW;
    this.maxHexPreviewBytes = options.maxHexPreviewBytes ?? DEFAULT_MAX_HEX_PREVIEW_BYTES;
    this.maxSortedKeys = options.maxSortedKeys ?? DEFAULT_MAX_SORTED_KEYS;
  }

  reset(): void {
    this.handles.clear();
    this.nextHandle = 1;
  }

  createListHandle(vars: Variable[]): number {
    return this.createHandle({ kind: 'list', vars });
  }

  getVariables(
    variablesReference: number,
    paging?: { start?: number; count?: number }
  ): Variable[] {
    const payload = this.handles.get(variablesReference);
    if (!payload) {
      return [];
    }

    const start = paging?.start;
    const count = paging?.count;
    const pagingRequested = typeof start === 'number' || typeof count === 'number';

    switch (payload.kind) {
      case 'list':
        return payload.vars;
      case 'string_full':
        return [{ name: '(full)', value: payload.full, type: 'string', variablesReference: 0 }];
      case 'bytes_details':
        return this.bytesDetailsToVariables(payload);
      case 'array': {
        const effectiveOffset = payload.offset + (typeof start === 'number' ? Math.max(0, start) : 0);
        const limit = typeof count === 'number' ? Math.max(0, count) : this.pageSize;
        return this.arrayToVariables(payload.items, effectiveOffset, limit, !pagingRequested);
      }
      case 'object': {
        const effectiveOffset = payload.offset + (typeof start === 'number' ? Math.max(0, start) : 0);
        const limit = typeof count === 'number' ? Math.max(0, count) : this.pageSize;
        return this.objectToVariables(payload.obj, payload.keys, effectiveOffset, limit, !pagingRequested);
      }
      default:
        return [];
    }
  }

  variablesFromArgs(args: string | undefined): Variable[] {
    if (!args) {
      return [this.toVariable('(args)', '(none)')];
    }

    try {
      const parsed = JSON.parse(args) as unknown;
      return this.childrenForValue(parsed);
    } catch {
      return [this.toVariable('(args)', args)];
    }
  }

  variablesFromStorage(storage: Record<string, unknown>): Variable[] {
    return Object.entries(storage)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, value]) => this.toVariable(name, value));
  }

  toVariable(name: string, value: unknown): Variable {
    if (value === null || value === undefined) {
      return { name, value: String(value), type: 'null', variablesReference: 0 };
    }

    if (typeof value === 'string') {
      return this.stringToVariable(name, value);
    }

    if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') {
      return { name, value: String(value), type: typeof value, variablesReference: 0 };
    }

    if (Array.isArray(value)) {
      const ref = this.createHandle({ kind: 'array', items: value, offset: 0 });
      return {
        name,
        value: `Array(${value.length})`,
        type: 'array',
        variablesReference: ref,
        indexedVariables: value.length
      };
    }

    if (typeof value === 'object') {
      if (isTypedAnnotationObject(value)) {
        return this.typedAnnotationToVariable(name, value);
      }

      const obj = value as Record<string, unknown>;
      const keys = Object.keys(obj);
      if (keys.length <= this.maxSortedKeys) {
        keys.sort((a, b) => a.localeCompare(b));
      }
      const ref = this.createHandle({ kind: 'object', obj, keys, offset: 0 });
      return {
        name,
        value: `Object(${keys.length})`,
        type: 'object',
        variablesReference: ref,
        namedVariables: keys.length
      };
    }

    return { name, value: String(value), type: typeof value, variablesReference: 0 };
  }

  private childrenForValue(value: unknown): Variable[] {
    if (Array.isArray(value)) {
      return this.arrayToVariables(value, 0, this.pageSize, true);
    }
    if (value && typeof value === 'object') {
      if (isTypedAnnotationObject(value)) {
        return [this.toVariable('(value)', value)];
      }

      const obj = value as Record<string, unknown>;
      const keys = Object.keys(obj);
      if (keys.length <= this.maxSortedKeys) {
        keys.sort((a, b) => a.localeCompare(b));
      }
      return this.objectToVariables(obj, keys, 0, this.pageSize, true);
    }
    return [this.toVariable('(value)', value)];
  }

  private stringToVariable(name: string, value: string): Variable {
    const { preview, truncated } = truncatePreview(value, this.maxStringPreview);
    if (!truncated) {
      return {
        name,
        value,
        type: looksLikeStrkeyAddress(value) ? 'address' : 'string',
        variablesReference: 0
      };
    }

    const ref = this.createHandle({ kind: 'string_full', full: value });
    return {
      name,
      value: `${preview} (truncated, expand)`,
      type: looksLikeStrkeyAddress(value) ? 'address' : 'string',
      variablesReference: ref
    };
  }

  private typedAnnotationToVariable(name: string, obj: Record<string, unknown>): Variable {
    const typeName = String(obj.type);
    const rawValue = obj.value;

    if ((typeName === 'bytes' || typeName === 'bytesn') && typeof rawValue === 'string') {
      const bytes = decodeBytesString(rawValue);
      if (bytes) {
        const previewBytes = bytes.slice(0, Math.max(0, this.maxHexPreviewBytes));
        const hexPreview = bytesToHex(previewBytes) + (bytes.length > previewBytes.length ? '…' : '');
        const detailsRef = this.createHandle({ kind: 'bytes_details', original: rawValue, bytes });

        const label = typeName === 'bytesn' && typeof obj.length === 'number'
          ? `bytesn(${obj.length})`
          : `bytes(${bytes.length})`;

        return {
          name,
          value: `${label} ${hexPreview} (expand for details)`,
          type: typeName,
          variablesReference: detailsRef
        };
      }
    }

    if (typeName === 'address' && typeof rawValue === 'string') {
      return {
        name,
        value: `address ${rawValue}`,
        type: 'address',
        variablesReference: 0
      };
    }

    // Fall back to rendering the annotation object as an expandable object with a compact summary.
    const keys = Object.keys(obj);
    if (keys.length <= this.maxSortedKeys) {
      keys.sort((a, b) => a.localeCompare(b));
    }
    const ref = this.createHandle({ kind: 'object', obj, keys, offset: 0 });
    return {
      name,
      value: `${typeName}`,
      type: 'typed',
      variablesReference: ref,
      namedVariables: keys.length
    };
  }

  private bytesDetailsToVariables(payload: BytesDetailsHandle): Variable[] {
    const { bytes, original } = payload;

    const hex = bytesToHex(bytes);
    const b64 = bytesToBase64(bytes);
    const utf8 = Buffer.from(bytes).toString('utf8');

    return [
      { name: 'original', value: original, type: 'string', variablesReference: 0 },
      { name: 'length', value: String(bytes.length), type: 'number', variablesReference: 0 },
      this.stringToVariable('hex', hex),
      this.stringToVariable('base64', b64),
      this.stringToVariable('utf8', utf8)
    ];
  }

  private arrayToVariables(
    items: unknown[],
    offset: number,
    limit: number,
    includeShowMore: boolean
  ): Variable[] {
    const start = Math.min(Math.max(0, offset), items.length);
    const end = Math.min(items.length, start + Math.max(0, limit));

    const vars: Variable[] = [];
    for (let i = start; i < end; i += 1) {
      vars.push(this.toVariable(`[${i}]`, items[i]));
    }

    if (includeShowMore && end < items.length) {
      const nextRef = this.createHandle({ kind: 'array', items, offset: end });
      vars.push({
        name: '… show more',
        value: `${end}/${items.length}`,
        type: 'pager',
        variablesReference: nextRef
      });
    }

    return vars;
  }

  private objectToVariables(
    obj: Record<string, unknown>,
    keys: string[],
    offset: number,
    limit: number,
    includeShowMore: boolean
  ): Variable[] {
    const start = Math.min(Math.max(0, offset), keys.length);
    const end = Math.min(keys.length, start + Math.max(0, limit));

    const vars: Variable[] = [];
    for (let i = start; i < end; i += 1) {
      const key = keys[i];
      vars.push(this.toVariable(key, obj[key]));
    }

    if (includeShowMore && end < keys.length) {
      const nextRef = this.createHandle({ kind: 'object', obj, keys, offset: end });
      vars.push({
        name: '… show more',
        value: `${end}/${keys.length}`,
        type: 'pager',
        variablesReference: nextRef
      });
    }

    return vars;
  }

  private createHandle(payload: HandlePayload): number {
    const ref = this.nextHandle++;
    this.handles.set(ref, payload);
    return ref;
  }
}

export const __testUtils = {
  looksLikeStrkeyAddress,
  decodeBytesString,
  safeStringify
};
