#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-only
// The App Store side of the store run (.github/workflows/store-release.yml, auto-deploy design 7-2)
// and the iOS build job's first step, through the App Store Connect API. Node 20 or later, Node's
// built-in modules only.
//
//   node scripts/asc-store.mjs status    [--version V] [--build B] [--json FILE]
//   node scripts/asc-store.mjs check-new --version V                      (client-build.yml, iOS job)
//   node scripts/asc-store.mjs prepare   --version V --build B [--release-type manual|after-approval] [--phased]
//   node scripts/asc-store.mjs submit    --version V --build B [--release-type ...] [--phased]
//   node scripts/asc-store.mjs release   --version V [--build B]
//   any of them with --dry-run
//
// status     TestFlight's processing of the version's builds, the App Store versions and their
//            states (appVersionState), the open review submissions with the versions in them, the
//            phased release: one summary line, and with --json a file. Read only.
// check-new  fails when App Store Connect has a build of this version already: the build run then
//            stops before it builds (a version goes to TestFlight once).
// prepare    finds the build (processed VALID, unexpired; waits up to ASC_WAIT_MINUTES, default 60),
//            finds the editable App Store version or renumbers the editable one or creates it, and
//            puts in store/appstore's texts (the version's and the app information's), the build,
//            the release type, the phased release and the App Review information. It never
//            submits. The first App Store version gets no What's New (App Store Connect has no such
//            field for it).
// submit     what prepare does, then the review submission: an open submission that has this
//            version waiting for or in review is success; a READY_FOR_REVIEW or UNRESOLVED_ISSUES
//            submission is used again (the item added if it is missing); one that holds other items
//            stops the run; only without an open submission a new one is made. Then submitted=true.
// release    releases an approved version of manual release (PENDING_DEVELOPER_RELEASE); a version
//            already PROCESSING_FOR_DISTRIBUTION or READY_FOR_DISTRIBUTION is success.
// --dry-run  without the key: no network, prints what the command would do from store/. With the
//            key: only GET requests, then every write it would make, with sizes and field names
//            in place of the secret values (Apple has no API that checks a submission beforehand).
//
// Environment: ASC_APP_ID, ASC_ISSUER_ID, ASC_KEY_ID, ASC_PRIVATE_KEY (the .p8 PEM). The store run
// passes its App Manager key (ASC_STORE_*) under these names, the build job its upload key. The
// review information comes from REVIEW_DEMO_USER, REVIEW_DEMO_PASSWORD, REVIEW_DEMO_PC_ID,
// REVIEW_DEMO_PC_PASSWORD and REVIEW_CONTACT_FIRST_NAME, _LAST_NAME, _PHONE, _EMAIL.
// Nothing secret is printed: not the key, the token, the demo values or the filled review notes
// (their byte count only, never a hash: the template is public, so a hash would let a reader of the
// log guess a short demo PC password). Apple's error details are printed with those values masked.
//
// Exit status: 0 done (or already done), 1 failed or refused, 2 bad usage.
// Offline tests: node --test scripts/asc-store.test.mjs (scripts/ci.sh client)
import { createPrivateKey, sign } from 'node:crypto';
import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

export const API = 'https://api.appstoreconnect.apple.com';
const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
export const STORE = join(ROOT, 'store', 'appstore');

// scripts/check-store-text.py's limits for the filled review notes
export const NOTES_BYTES = 4000;
export const PC_ID_MAX = 16;
export const PC_PASSWORD_MAX = 32;
const PLACEHOLDERS = { DEMO_PC_ID: 'REVIEW_DEMO_PC_ID', DEMO_PC_PASSWORD: 'REVIEW_DEMO_PC_PASSWORD' };
export const REVIEW_ENV = ['REVIEW_DEMO_USER', 'REVIEW_DEMO_PASSWORD', 'REVIEW_DEMO_PC_ID', 'REVIEW_DEMO_PC_PASSWORD',
  'REVIEW_CONTACT_FIRST_NAME', 'REVIEW_CONTACT_LAST_NAME', 'REVIEW_CONTACT_PHONE', 'REVIEW_CONTACT_EMAIL'];

// AppVersionState
export const EDITABLE = ['PREPARE_FOR_SUBMISSION', 'READY_FOR_REVIEW', 'DEVELOPER_REJECTED', 'REJECTED',
  'METADATA_REJECTED', 'INVALID_BINARY'];
const IN_REVIEW = ['WAITING_FOR_REVIEW', 'IN_REVIEW', 'WAITING_FOR_EXPORT_COMPLIANCE'];
const APPROVED = ['ACCEPTED', 'PENDING_DEVELOPER_RELEASE', 'PENDING_APPLE_RELEASE'];
const RELEASED = ['PROCESSING_FOR_DISTRIBUTION', 'READY_FOR_DISTRIBUTION'];
const ON_STORE_ONCE = [...RELEASED, 'REPLACED_WITH_NEW_VERSION'];
// ReviewSubmission states that are open (Apple keeps one open submission per app and platform)
const OPEN_SUBMISSION = ['READY_FOR_REVIEW', 'WAITING_FOR_REVIEW', 'IN_REVIEW', 'UNRESOLVED_ISSUES'];
// AppInfo states whose localizations can be changed
const APP_INFO_EDITABLE = ['PREPARE_FOR_SUBMISSION', 'READY_FOR_REVIEW', 'DEVELOPER_REJECTED', 'REJECTED'];

const VERSION = /^(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})$/;
const BUILD = /^\d{1,18}$/;
const COMMANDS = ['status', 'check-new', 'prepare', 'submit', 'release'];
const TOKEN_LIFETIME = 1200; // Apple's limit for a token's lifetime: 20 minutes
const TOKEN_RENEW = 600;

export class AscError extends Error {
  constructor(message, { usage = false, hint = '' } = {}) {
    super(message);
    this.usage = usage;
    this.hint = hint;
  }
}

// ---- input ----------------------------------------------------------------------------------------

export function parseCommand(argv) {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv, allowPositionals: true, strict: true, options: {
        version: { type: 'string' }, build: { type: 'string' }, json: { type: 'string' },
        'release-type': { type: 'string', default: 'manual' }, phased: { type: 'boolean', default: false },
        'dry-run': { type: 'boolean', default: false }, store: { type: 'string' },
      },
    });
  } catch (error) {
    throw new AscError(error.message, { usage: true });
  }
  const [command, ...rest] = parsed.positionals;
  const o = parsed.values;
  if (!COMMANDS.includes(command) || rest.length) throw new AscError(`the command is one of ${COMMANDS.join(', ')}`, { usage: true });
  if (o.version !== undefined && !VERSION.test(o.version)) throw new AscError(`not a version X.Y.Z: ${o.version}`, { usage: true });
  if (o.build !== undefined && !BUILD.test(o.build)) throw new AscError(`not a build number: ${o.build}`, { usage: true });
  if (command !== 'status' && !o.version) throw new AscError(`${command} needs --version`, { usage: true });
  if (['prepare', 'submit'].includes(command) && !o.build) throw new AscError(`${command} needs --build`, { usage: true });
  if (!['manual', 'after-approval'].includes(o['release-type'])) throw new AscError('--release-type is manual or after-approval', { usage: true });
  return {
    command, version: o.version, build: o.build, json: o.json, phased: o.phased, dryRun: o['dry-run'],
    releaseType: o['release-type'] === 'manual' ? 'MANUAL' : 'AFTER_APPROVAL', store: o.store ?? STORE,
  };
}

function text(dir, rel) {
  const path = join(dir, rel);
  if (!existsSync(path)) throw new AscError(`store/appstore/${rel} is missing`);
  return readFileSync(path, 'utf8').replace(/\n$/, '');
}

// The texts of store/appstore (store/README.md); scripts/check-store-text.py checks them in the
// store run's checks job, this reads them.
export function readStore(dir, version) {
  const storeVersion = text(dir, 'version.txt');
  if (version && storeVersion !== version) {
    throw new AscError(`store/appstore/version.txt is ${storeVersion}, the run's version is ${version}`, { usage: true });
  }
  const locales = text(dir, 'locales.txt').split('\n').filter(Boolean);
  const texts = {};
  for (const locale of locales) {
    const t = (name) => text(dir, `${locale}/${name}.txt`);
    texts[locale] = {
      info: { name: t('name'), subtitle: t('subtitle'), privacyPolicyUrl: t('privacy_url') },
      version: {
        description: t('description'), keywords: t('keywords'), promotionalText: t('promotional_text'),
        supportUrl: t('support_url'), marketingUrl: t('marketing_url') || null, whatsNew: t('whats_new'),
      },
    };
  }
  return {
    version: storeVersion, distribution: text(dir, 'distribution.txt'), copyright: text(dir, 'copyright.txt'),
    notes: text(dir, 'review_notes.txt'), locales, texts,
  };
}

// bytes as check-store-text.py counts them: UTF-8, every line break as CRLF
export const notesBytes = (notes) => Buffer.byteLength(notes, 'utf8') + (notes.match(/\n/g) ?? []).length;

const printable = /^[\x21-\x7e]+$/;

// The review notes filled from the environment, checked; the values are never part of a message.
export function fillNotes(template, env) {
  let notes = template;
  for (const [name, variable] of Object.entries(PLACEHOLDERS)) {
    const value = env[variable] ?? '';
    const limit = name === 'DEMO_PC_ID' ? PC_ID_MAX : PC_PASSWORD_MAX;
    if (!value) throw new AscError(`${variable} is not set`);
    if (!printable.test(value)) throw new AscError(`${variable} must be printable ASCII without spaces`);
    if (Buffer.byteLength(value) > limit) throw new AscError(`${variable} is longer than ${limit} bytes (check-store-text.py's worst case)`);
    const marker = `{{${name}}}`;
    if (notes.split(marker).length !== 2) throw new AscError(`store/appstore/review_notes.txt must hold ${marker} once`);
    notes = notes.replace(marker, () => value);
  }
  if (/\{\{|\}\}/.test(notes)) throw new AscError('store/appstore/review_notes.txt has a placeholder the run cannot fill');
  const bytes = notesBytes(notes);
  if (bytes > NOTES_BYTES) throw new AscError(`the filled review notes are ${bytes} bytes; App Store Connect takes ${NOTES_BYTES}`);
  return { notes, bytes };
}

export function reviewDetail(store, env) {
  for (const name of REVIEW_ENV) {
    if (!env[name]) throw new AscError(`${name} is not set (environment store, docs/store-release.md)`);
  }
  const { notes, bytes } = fillNotes(store.notes, env);
  return {
    bytes,
    attributes: {
      contactFirstName: env.REVIEW_CONTACT_FIRST_NAME, contactLastName: env.REVIEW_CONTACT_LAST_NAME,
      contactPhone: env.REVIEW_CONTACT_PHONE, contactEmail: env.REVIEW_CONTACT_EMAIL,
      demoAccountName: env.REVIEW_DEMO_USER, demoAccountPassword: env.REVIEW_DEMO_PASSWORD,
      demoAccountRequired: true, notes,
    },
  };
}

// ---- the token and the calls ------------------------------------------------------------------------

const b64 = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');

export function createToken({ keyId, issuerId, privateKey }, now = Math.floor(Date.now() / 1000)) {
  let key;
  try { key = createPrivateKey(privateKey); } catch { throw new AscError('ASC_PRIVATE_KEY is not a private key (the .p8 file)', { usage: true }); }
  if (key.asymmetricKeyType !== 'ec' || key.asymmetricKeyDetails?.namedCurve !== 'prime256v1') {
    throw new AscError('ASC_PRIVATE_KEY is not an App Store Connect API key (P-256)', { usage: true });
  }
  const input = `${b64({ alg: 'ES256', kid: keyId, typ: 'JWT' })}.${b64({ iss: issuerId, iat: now, exp: now + TOKEN_LIFETIME, aud: 'appstoreconnect-v1' })}`;
  return `${input}.${sign('sha256', Buffer.from(input), { key, dsaEncoding: 'ieee-p1363' }).toString('base64url')}`;
}

export function credentials(env) {
  const set = ['ASC_APP_ID', 'ASC_ISSUER_ID', 'ASC_KEY_ID', 'ASC_PRIVATE_KEY'].filter((n) => env[n]);
  if (set.length === 0) return null;
  if (set.length < 4) throw new AscError(`set ASC_APP_ID, ASC_ISSUER_ID, ASC_KEY_ID and ASC_PRIVATE_KEY together (missing: ${['ASC_APP_ID', 'ASC_ISSUER_ID', 'ASC_KEY_ID', 'ASC_PRIVATE_KEY'].filter((n) => !env[n]).join(', ')})`, { usage: true });
  if (!/^\d+$/.test(env.ASC_APP_ID)) throw new AscError('ASC_APP_ID is not the app\'s numeric Apple ID', { usage: true });
  if (!/^[A-Z0-9]{6,20}$/.test(env.ASC_KEY_ID)) throw new AscError('ASC_KEY_ID is not a key ID', { usage: true });
  if (!/^[0-9a-f-]{36}$/i.test(env.ASC_ISSUER_ID)) throw new AscError('ASC_ISSUER_ID is not an issuer ID', { usage: true });
  return { appId: env.ASC_APP_ID, keyId: env.ASC_KEY_ID, issuerId: env.ASC_ISSUER_ID, privateKey: env.ASC_PRIVATE_KEY.replace(/\\n/g, '\n') };
}

function mask(message, secrets) {
  let out = String(message ?? '');
  for (const s of secrets) if (s && s.length >= 3) out = out.split(s).join('<masked>');
  return out.slice(0, 400);
}

export class Client {
  constructor({ creds, fetch, sleep, now, log, dryRun, secrets }) {
    Object.assign(this, { creds, fetch, sleep, now, log, dryRun, secrets });
    this.token = null;
    this.tokenAt = 0;
  }

  bearer() {
    const t = Math.floor(this.now() / 1000);
    if (!this.token || t - this.tokenAt > TOKEN_RENEW) {
      this.token = createToken(this.creds, t);
      this.tokenAt = t;
    }
    return this.token;
  }

  async call(method, path, body) {
    const url = path.startsWith('https://') ? path : `${API}${path}`;
    if (!url.startsWith(`${API}/`)) throw new AscError('App Store Connect answered with a link to another host');
    for (let attempt = 1; ; attempt += 1) {
      let response;
      try {
        response = await this.fetch(url, {
          method, redirect: 'error', signal: AbortSignal.timeout(60000),
          headers: { Authorization: `Bearer ${this.bearer()}`, Accept: 'application/json', ...(body ? { 'Content-Type': 'application/json' } : {}) },
          body: body ? JSON.stringify(body) : undefined,
        });
      } catch (error) {
        if (error instanceof AscError) throw error;
        response = null;
      }
      const status = response?.status ?? 0;
      if ((status === 0 || status === 429 || status >= 500) && method === 'GET' && attempt < 3) {
        await this.sleep(5000 * attempt);
        continue;
      }
      if (status === 0) {
        throw new AscError(`App Store Connect did not answer ${method} ${url.split('?')[0].slice(API.length)}`,
          { hint: method === 'GET' ? '' : 'The change may or may not have been made: run the same command again; it reads the state first.' });
      }
      let payload = {};
      if (status !== 204) {
        try { payload = await response.json(); } catch { payload = {}; }
      }
      if (status >= 200 && status < 300) return payload ?? {};
      const errors = Array.isArray(payload?.errors) ? payload.errors : [];
      const detail = errors.map((e) => `${e.code ?? ''} ${e.title ?? ''}: ${e.detail ?? ''}`.trim()).join('; ');
      const error = new AscError(`App Store Connect answered ${method} ${url.split('?')[0].slice(API.length)} with HTTP ${status}${detail ? `: ${mask(detail, this.secrets)}` : ''}`);
      error.status = status;
      error.codes = errors.map((e) => String(e.code ?? ''));
      if (status === 401) error.hint = 'The key was refused: check ASC_KEY_ID, ASC_ISSUER_ID and ASC_PRIVATE_KEY (a revoked key answers 401).';
      else if (status === 403) error.hint = 'The key has no access to this, or an agreement waits in App Store Connect > Business (only the Account Holder can accept it); the store run needs an App Manager key.';
      else if (status === 409) error.hint = 'App Store Connect refused the change in this state; the message above says why.';
      throw error;
    }
  }

  get(path) { return this.call('GET', path); }

  // a to-one relationship: its data, or null when there is none (App Store Connect answers
  // {"data": null}, or 404 for some of them)
  async one(path) {
    try {
      return (await this.get(path)).data ?? null;
    } catch (error) {
      if (error.status === 404) return null;
      throw error;
    }
  }

  async all(path) {
    const data = [];
    const included = [];
    let next = path;
    for (let page = 0; next && page < 20; page += 1) {
      const answer = await this.get(next);
      if (!Array.isArray(answer.data)) throw new AscError(`App Store Connect answered ${path.split('?')[0]} without a list`);
      data.push(...answer.data);
      included.push(...(answer.included ?? []));
      next = answer.links?.next ?? null;
    }
    return { data, included };
  }

  // a write: done, or printed in a dry run (with sizes, never secret values)
  async write(method, path, body, what) {
    if (this.dryRun) {
      this.log(`would ${method} ${path}: ${what}`);
      return null;
    }
    const answer = await this.call(method, path, body);
    this.log(`${method} ${path}: ${what}`);
    return answer;
  }
}

const q = (params) => new URLSearchParams(params).toString();

// ---- reading App Store Connect ----------------------------------------------------------------------

export async function listBuilds(client, appId, { version, build, limit = 5 } = {}) {
  const params = {
    'filter[app]': appId, 'filter[preReleaseVersion.platform]': 'IOS', sort: '-uploadedDate', limit: String(limit),
    'fields[builds]': 'version,processingState,expired,expirationDate,uploadedDate,preReleaseVersion',
    include: 'preReleaseVersion', 'fields[preReleaseVersions]': 'version,platform',
  };
  if (version) params['filter[preReleaseVersion.version]'] = version;
  if (build) params['filter[version]'] = build;
  const answer = await client.get(`/v1/builds?${q(params)}`);
  if (!Array.isArray(answer.data)) throw new AscError('App Store Connect answered /v1/builds without a list');
  const versions = new Map((answer.included ?? []).filter((i) => i.type === 'preReleaseVersions').map((i) => [i.id, i.attributes?.version]));
  return answer.data.map((b) => ({
    id: b.id, build: b.attributes?.version, state: b.attributes?.processingState, expired: b.attributes?.expired === true,
    expirationDate: b.attributes?.expirationDate ?? null, uploadedDate: b.attributes?.uploadedDate ?? null,
    version: versions.get(b.relationships?.preReleaseVersion?.data?.id) ?? version ?? null,
  }));
}

async function versionsOf(client, appId) {
  const { data } = await client.all(`/v1/apps/${appId}/appStoreVersions?${q({
    'filter[platform]': 'IOS', 'fields[appStoreVersions]': 'versionString,appVersionState,appStoreState,releaseType,copyright', limit: '200',
  })}`);
  return data.map((v) => ({
    id: v.id, version: v.attributes?.versionString, state: v.attributes?.appVersionState ?? v.attributes?.appStoreState ?? 'UNKNOWN',
    releaseType: v.attributes?.releaseType ?? null, copyright: v.attributes?.copyright ?? null,
  }));
}

async function openSubmissions(client, appId) {
  const { data } = await client.all(`/v1/apps/${appId}/reviewSubmissions?${q({
    'filter[platform]': 'IOS', 'filter[state]': OPEN_SUBMISSION.join(','), limit: '20',
  })}`);
  const out = [];
  for (const s of data) {
    const items = await client.all(`/v1/reviewSubmissions/${s.id}/items?${q({ include: 'appStoreVersion', limit: '50' })}`);
    out.push({
      id: s.id, state: s.attributes?.state,
      items: items.data.map((i) => ({ id: i.id, versionId: i.relationships?.appStoreVersion?.data?.id ?? null })),
    });
  }
  return out;
}

const highest = (list) => list.map((v) => v.version).filter((v) => VERSION.test(v ?? ''))
  .sort((a, b) => { const x = a.split('.').map(Number); const y = b.split('.').map(Number); return x[0] - y[0] || x[1] - y[1] || x[2] - y[2]; }).pop() ?? null;

// ---- the commands -----------------------------------------------------------------------------------

async function status(ctx) {
  const { client, creds, opts, log } = ctx;
  const builds = await listBuilds(client, creds.appId, { version: opts.version, build: opts.build });
  const versions = await versionsOf(client, creds.appId);
  const submissions = await openSubmissions(client, creds.appId);
  const byId = new Map(versions.map((v) => [v.id, v.version]));
  const inReview = submissions.filter((s) => s.state !== 'READY_FOR_REVIEW').flatMap((s) => s.items.map((i) => byId.get(i.versionId))).filter(Boolean);
  const target = opts.version ? versions.find((v) => v.version === opts.version) : null;
  let phased = null;
  if (target) {
    const data = await client.one(`/v1/appStoreVersions/${target.id}/appStoreVersionPhasedRelease`);
    phased = data ? { state: data.attributes?.phasedReleaseState ?? null, day: data.attributes?.currentDayNumber ?? null } : null;
  }
  const report = {
    checked_at: new Date(ctx.now()).toISOString(), version: opts.version ?? null,
    testflight: builds, app_store_versions: versions,
    open_submissions: submissions.map((s) => ({ id: s.id, state: s.state, versions: s.items.map((i) => byId.get(i.versionId) ?? null) })),
    in_review: [...new Set(inReview)],
    on_app_store: highest(versions.filter((v) => v.state === 'READY_FOR_DISTRIBUTION')),
    phased_release: phased,
  };
  for (const b of builds) log(`TestFlight ${b.version} (${b.build}): ${b.state}${b.expired ? ', expired' : ''}`);
  for (const v of versions) log(`App Store version ${v.version}: ${v.state} (${v.releaseType ?? '-'})`);
  for (const s of report.open_submissions) log(`review submission ${s.state}: ${s.versions.join(', ') || 'no version'}`);
  const tf = builds[0] ? `TestFlight ${builds[0].version} (${builds[0].build}) ${builds[0].state}` : 'TestFlight: no build';
  const line = `App Store: on the App Store ${report.on_app_store ?? 'none'}; in review ${report.in_review.join(', ') || 'none'}; `
    + (target ? `${target.version} ${target.state}${phased ? `, phased ${phased.state}` : ''}; ` : '') + tf;
  ctx.summary(line);
  if (opts.json) writeFileSync(opts.json, `${JSON.stringify(report, null, 2)}\n`);
  return 0;
}

async function checkNew(ctx) {
  const { client, creds, opts } = ctx;
  const builds = await listBuilds(client, creds.appId, { version: opts.version, limit: 1 });
  if (builds.length) {
    throw new AscError(`App Store Connect has iOS ${opts.version} already (build ${builds[0].build}, ${builds[0].state})`,
      { hint: 'A version goes to TestFlight once: raise client/version.env for a new build. After a failed job whose upload went through, see the store run status (docs/store-release.md).' });
  }
  ctx.log(`App Store Connect has no iOS ${opts.version} build yet`);
  return 0;
}

async function waitForBuild(ctx) {
  const { client, creds, opts } = ctx;
  const deadline = ctx.now() + ctx.waitMinutes * 60000;
  for (;;) {
    const [b] = await listBuilds(client, creds.appId, { version: opts.version, build: opts.build, limit: 2 });
    if (b) {
      if (b.build !== opts.build || b.version !== opts.version) throw new AscError(`App Store Connect answered build ${b.version} (${b.build}) for ${opts.version} (${opts.build})`);
      if (['FAILED', 'INVALID'].includes(b.state)) throw new AscError(`iOS ${opts.version} (${opts.build}) is ${b.state} in App Store Connect`, { hint: 'Build again with a higher version.' });
      if (b.expired) throw new AscError(`iOS ${opts.version} (${opts.build}) has expired`);
      if (b.state === 'VALID') return b;
    }
    if (ctx.dryRun || ctx.now() >= deadline) {
      throw new AscError(b ? `iOS ${opts.version} (${opts.build}) is still ${b.state}` : `App Store Connect has no iOS ${opts.version} build ${opts.build}`,
        { hint: ctx.dryRun ? 'A dry run does not wait.' : `waited ${ctx.waitMinutes} minutes; run it again once TestFlight shows the build` });
    }
    ctx.log(`iOS ${opts.version} (${opts.build}): ${b ? b.state : 'not visible yet'}; waiting`);
    await ctx.sleep(60000);
  }
}

const sizes = (attrs) => Object.entries(attrs).map(([k, v]) => `${k} (${v === null ? 'empty' : `${[...String(v)].length} chars`})`).join(', ');

function changed(current, wanted) {
  return Object.fromEntries(Object.entries(wanted).filter(([k, v]) => (current?.[k] ?? null) !== v));
}

// The editable App Store version of opts.version: found, renumbered or made. Returns it and whether
// the app has never had a version on the App Store.
async function editableVersion(ctx, versions) {
  const { client, creds, opts } = ctx;
  const first = !versions.some((v) => ON_STORE_ONCE.includes(v.state));
  const same = versions.find((v) => v.version === opts.version);
  if (same) {
    if (!EDITABLE.includes(same.state)) {
      throw new AscError(`App Store version ${opts.version} is ${same.state}: it cannot be changed in that state`,
        { hint: IN_REVIEW.includes(same.state) ? 'It is with App Review; wait for the result (status).' : 'A new build goes with a new version (the owner\'s rule).' });
    }
    return { version: same, first };
  }
  const other = versions.find((v) => EDITABLE.includes(v.state));
  if (other) {
    await client.write('PATCH', `/v1/appStoreVersions/${other.id}`,
      { data: { type: 'appStoreVersions', id: other.id, attributes: { versionString: opts.version } } },
      `renumber the editable version ${other.version} (${other.state}) to ${opts.version}`);
    return { version: { ...other, version: opts.version }, first };
  }
  const busy = versions.find((v) => [...IN_REVIEW, ...APPROVED].includes(v.state));
  if (busy) {
    throw new AscError(`App Store version ${busy.version} is ${busy.state}: App Store Connect takes no new version until it is released or rejected`);
  }
  const created = await client.write('POST', '/v1/appStoreVersions', {
    data: {
      type: 'appStoreVersions', attributes: { platform: 'IOS', versionString: opts.version, releaseType: opts.releaseType },
      relationships: { app: { data: { type: 'apps', id: creds.appId } } },
    },
  }, `create App Store version ${opts.version} (${opts.releaseType})`);
  return { version: { id: created?.data?.id ?? '<new version>', version: opts.version, state: 'PREPARE_FOR_SUBMISSION', releaseType: null, copyright: null, created: true }, first };
}

async function prepare(ctx) {
  const { client, creds, opts, store, log } = ctx;
  // the review information first: a missing secret stops the run before anything is written
  const review = ctx.dryRun && !REVIEW_ENV.every((n) => ctx.env[n]) ? null : reviewDetail(store, ctx.env);
  const build = await waitForBuild(ctx);
  log(`build: iOS ${build.version} (${build.build}) ${build.state}, expires ${build.expirationDate ?? '?'}`);
  const versions = await versionsOf(client, creds.appId);
  const { version, first } = await editableVersion(ctx, versions);
  const vid = version.id;
  // a version this dry run would create has nothing to read yet
  const fresh = Boolean(version.created) && ctx.dryRun;
  if (first) log('first App Store version: no What\'s New is sent');

  const attrs = changed(version, { copyright: store.copyright, releaseType: opts.releaseType });
  if (Object.keys(attrs).length) {
    await client.write('PATCH', `/v1/appStoreVersions/${vid}`, { data: { type: 'appStoreVersions', id: vid, attributes: attrs } },
      `set ${Object.entries(attrs).map(([k, v]) => `${k} ${v}`).join(', ')}`);
  }

  const current = fresh ? null : await client.one(`/v1/appStoreVersions/${vid}/build`);
  if (current?.id !== build.id) {
    await client.write('PATCH', `/v1/appStoreVersions/${vid}/relationships/build`, { data: { type: 'builds', id: build.id } },
      `attach build ${build.build}${current ? ' (in place of another build)' : ''}`);
  }

  // the version page's texts
  const locs = fresh ? [] : (await client.all(`/v1/appStoreVersions/${vid}/appStoreVersionLocalizations?limit=50`)).data;
  for (const locale of store.locales) {
    const wanted = { ...store.texts[locale].version };
    if (first) delete wanted.whatsNew;
    const have = locs.find((l) => l.attributes?.locale === locale);
    if (have) {
      const diff = changed(have.attributes, wanted);
      if (Object.keys(diff).length) {
        await client.write('PATCH', `/v1/appStoreVersionLocalizations/${have.id}`,
          { data: { type: 'appStoreVersionLocalizations', id: have.id, attributes: diff } }, `${locale}: ${sizes(diff)}`);
      } else log(`${locale}: the version page's texts are store/'s already`);
    } else {
      await client.write('POST', '/v1/appStoreVersionLocalizations', {
        data: { type: 'appStoreVersionLocalizations', attributes: { locale, ...wanted }, relationships: { appStoreVersion: { data: { type: 'appStoreVersions', id: vid } } } },
      }, `add ${locale}: ${sizes(wanted)}`);
    }
  }

  // the app information's texts (name, subtitle, privacy policy URL)
  const infos = (await client.all(`/v1/apps/${creds.appId}/appInfos?limit=10`)).data;
  const info = infos.find((i) => APP_INFO_EDITABLE.includes(i.attributes?.state ?? i.attributes?.appStoreState));
  if (!info) {
    log('note: no app information is editable now (name, subtitle and privacy URL change with the next version)');
  } else {
    const infoLocs = (await client.all(`/v1/appInfos/${info.id}/appInfoLocalizations?limit=50`)).data;
    for (const locale of store.locales) {
      const wanted = store.texts[locale].info;
      const have = infoLocs.find((l) => l.attributes?.locale === locale);
      if (have) {
        const diff = changed(have.attributes, wanted);
        if (Object.keys(diff).length) {
          await client.write('PATCH', `/v1/appInfoLocalizations/${have.id}`, { data: { type: 'appInfoLocalizations', id: have.id, attributes: diff } },
            `${locale} app information: ${sizes(diff)}`);
        } else log(`${locale}: the app information's texts are store/'s already`);
      } else {
        await client.write('POST', '/v1/appInfoLocalizations', {
          data: { type: 'appInfoLocalizations', attributes: { locale, ...wanted }, relationships: { appInfo: { data: { type: 'appInfos', id: info.id } } } },
        }, `add ${locale} app information: ${sizes(wanted)}`);
      }
    }
  }

  // App Review information: the demo values go to the Sign-In Information and the notes, never to the log
  const detail = fresh ? null : await client.one(`/v1/appStoreVersions/${vid}/appStoreReviewDetail`);
  if (!review) {
    log(`would ${detail ? 'PATCH' : 'POST'} the App Review information: the notes (template ${notesBytes(store.notes)} bytes) and the sign-in information are filled from REVIEW_* (not set in this dry run)`);
  } else {
    const what = `App Review information: contact, demo account (required), notes (${review.bytes} bytes filled)`;
    if (detail) {
      const diff = changed(detail.attributes, review.attributes);
      if (Object.keys(diff).length) {
        await client.write('PATCH', `/v1/appStoreReviewDetails/${detail.id}`, { data: { type: 'appStoreReviewDetails', id: detail.id, attributes: diff } },
          `${what}; changed: ${Object.keys(diff).join(', ')}`);
      } else log('the App Review information is the environment\'s already');
    } else {
      await client.write('POST', '/v1/appStoreReviewDetails', {
        data: { type: 'appStoreReviewDetails', attributes: review.attributes, relationships: { appStoreVersion: { data: { type: 'appStoreVersions', id: vid } } } },
      }, what);
    }
  }

  // the phased release (after-approval with phased: design 9 #6)
  const phased = fresh ? null : await client.one(`/v1/appStoreVersions/${vid}/appStoreVersionPhasedRelease`);
  if (opts.phased && !phased) {
    await client.write('POST', '/v1/appStoreVersionPhasedReleases', {
      data: { type: 'appStoreVersionPhasedReleases', attributes: { phasedReleaseState: 'INACTIVE' }, relationships: { appStoreVersion: { data: { type: 'appStoreVersions', id: vid } } } },
    }, 'phased release on (starts when the version is released)');
  } else if (!opts.phased && phased) {
    if (phased.attributes?.phasedReleaseState === 'INACTIVE') {
      await client.write('DELETE', `/v1/appStoreVersionPhasedReleases/${phased.id}`, undefined, 'phased release off');
    } else log(`note: the phased release is ${phased.attributes?.phasedReleaseState}; left as it is`);
  }
  return { vid, build, first };
}

async function prepareCommand(ctx) {
  await prepare(ctx);
  ctx.summary(`${ctx.dryRun ? 'Dry run: nothing was changed. ' : ''}App Store version ${ctx.opts.version} (${ctx.opts.build}) prepared; it was not submitted.`
    + (ctx.dryRun ? '' : ' Check it in App Store Connect before submit (docs/store-release.md).'));
  return 0;
}

async function submit(ctx) {
  const { client, creds, opts, store } = ctx;
  const versions = await versionsOf(client, creds.appId);
  const same = versions.find((v) => v.version === opts.version);
  if (same && [...IN_REVIEW, ...APPROVED, ...RELEASED].includes(same.state)) {
    ctx.summary(`App Store version ${opts.version} is ${same.state}: submitted already; nothing to do.`);
    return 0;
  }
  const byId = new Map(versions.map((v) => [v.id, v.version]));
  const open = await openSubmissions(client, creds.appId);
  for (const s of open) {
    const others = s.items.filter((i) => byId.get(i.versionId) !== opts.version);
    const ours = s.items.some((i) => byId.get(i.versionId) === opts.version);
    if (ours && ['WAITING_FOR_REVIEW', 'IN_REVIEW'].includes(s.state)) {
      ctx.summary(`App Store version ${opts.version} is in a submission that is ${s.state}: submitted already; nothing to do.`);
      return 0;
    }
    if (others.length) {
      throw new AscError(`an open review submission (${s.state}) holds ${others.length} other item(s): ${others.map((i) => byId.get(i.versionId) ?? 'not a version').join(', ')}`,
        { hint: 'Apple keeps one open submission per app: finish or remove those items in App Store Connect first. The run cancels nothing.' });
    }
  }
  const { vid } = await prepare(ctx);
  let submission = open.find((s) => ['READY_FOR_REVIEW', 'UNRESOLVED_ISSUES'].includes(s.state));
  if (submission) {
    ctx.log(`using the open review submission (${submission.state}) again`);
  } else {
    const made = await client.write('POST', '/v1/reviewSubmissions', {
      data: { type: 'reviewSubmissions', attributes: { platform: 'IOS' }, relationships: { app: { data: { type: 'apps', id: creds.appId } } } },
    }, 'create a review submission');
    submission = { id: made?.data?.id ?? '<new submission>', items: [], state: 'READY_FOR_REVIEW' };
  }
  if (!submission.items.some((i) => i.versionId === vid)) {
    await client.write('POST', '/v1/reviewSubmissionItems', {
      data: {
        type: 'reviewSubmissionItems',
        relationships: { reviewSubmission: { data: { type: 'reviewSubmissions', id: submission.id } }, appStoreVersion: { data: { type: 'appStoreVersions', id: vid } } },
      },
    }, `add App Store version ${opts.version} to the submission`);
  }
  await client.write('PATCH', `/v1/reviewSubmissions/${submission.id}`,
    { data: { type: 'reviewSubmissions', id: submission.id, attributes: { submitted: true } } }, 'submit for App Review');
  const unlisted = store.distribution === 'unlisted' ? ' Next: the unlisted app request (docs/store-release.md).' : '';
  ctx.summary(ctx.dryRun ? `Dry run: nothing was changed or submitted (iOS ${opts.version}, build ${opts.build}).`
    : `Submitted App Store version ${opts.version} (build ${opts.build}) for App Review (${opts.releaseType === 'MANUAL' ? 'manual release' : 'released after approval'}${opts.phased ? ', phased' : ''}).${unlisted}`);
  return 0;
}

async function release(ctx) {
  const { client, creds, opts } = ctx;
  const versions = await versionsOf(client, creds.appId);
  const v = versions.find((x) => x.version === opts.version);
  if (!v) throw new AscError(`App Store Connect has no App Store version ${opts.version}`);
  if (RELEASED.includes(v.state)) {
    ctx.summary(`App Store version ${opts.version} is ${v.state}: released already; nothing to do.`);
    return 0;
  }
  if (v.state !== 'PENDING_DEVELOPER_RELEASE') {
    throw new AscError(`App Store version ${opts.version} is ${v.state}: only an approved version of manual release (PENDING_DEVELOPER_RELEASE) can be released`);
  }
  if (opts.build) {
    const b = await client.one(`/v1/appStoreVersions/${v.id}/build?${q({ 'fields[builds]': 'version' })}`);
    if (b?.attributes?.version !== opts.build) throw new AscError(`App Store version ${opts.version} has build ${b?.attributes?.version ?? 'none'}, not ${opts.build}`);
  }
  await client.write('POST', '/v1/appStoreVersionReleaseRequests', {
    data: { type: 'appStoreVersionReleaseRequests', relationships: { appStoreVersion: { data: { type: 'appStoreVersions', id: v.id } } } },
  }, `release App Store version ${opts.version}`);
  ctx.summary(ctx.dryRun ? `Dry run: App Store version ${opts.version} would be released.` : `Released App Store version ${opts.version}; status shows READY_FOR_DISTRIBUTION once Apple has processed it.`);
  return 0;
}

// a dry run without the key: what the command would do, from store/ alone
function offlinePlan(ctx) {
  const { opts, store, log } = ctx;
  log('dry run without App Store Connect credentials: no network; the plan from store/appstore');
  const plan = {
    status: ['GET /v1/builds (TestFlight processing)', 'GET /v1/apps/<app>/appStoreVersions', 'GET /v1/apps/<app>/reviewSubmissions and their items', 'GET the phased release'],
    'check-new': [`GET /v1/builds?filter[preReleaseVersion.version]=${opts.version}: fail when there is one`],
    release: [`GET /v1/apps/<app>/appStoreVersions: ${opts.version} PROCESSING_FOR_DISTRIBUTION or READY_FOR_DISTRIBUTION is done`, 'POST /v1/appStoreVersionReleaseRequests when it is PENDING_DEVELOPER_RELEASE'],
  }[opts.command];
  if (plan) { for (const step of plan) log(`would: ${step}`); return 0; }
  const steps = [
    `wait for build ${opts.build} of ${opts.version} to be VALID (up to ${ctx.waitMinutes} minutes)`,
    `find, renumber or create App Store version ${opts.version} (${opts.releaseType}); stop if it is not editable`,
    `set copyright "${store.copyright}", attach build ${opts.build}`,
    ...store.locales.map((l) => {
      const v = { ...store.texts[l].version };
      return `${l} version page: ${sizes(v)} (What's New only when the app has had an App Store version)`;
    }),
    ...store.locales.map((l) => `${l} app information: ${sizes(store.texts[l].info)}`),
    `App Review information: contact and demo account from REVIEW_* (${REVIEW_ENV.filter((n) => ctx.env[n]).length} of ${REVIEW_ENV.length} set here), notes from review_notes.txt (${notesBytes(store.notes)} bytes before filling)`,
    opts.phased ? 'phased release on' : 'no phased release',
  ];
  if (opts.command === 'submit') steps.push('use the open READY_FOR_REVIEW or UNRESOLVED_ISSUES submission or create one, add the version, submitted=true');
  for (const step of steps) log(`would: ${step}`);
  log(`distribution: ${store.distribution}`);
  return 0;
}

export async function run(argv, { env = process.env, fetch = globalThis.fetch, sleep = (ms) => new Promise((r) => setTimeout(r, ms)), now = Date.now, out = console.log } = {}) {
  const log = (line) => out(line);
  const summary = (line) => {
    log(line);
    if (env.GITHUB_STEP_SUMMARY) appendFileSync(env.GITHUB_STEP_SUMMARY, `${line}\n`);
  };
  const secretsOf = () => [env.ASC_PRIVATE_KEY, ...REVIEW_ENV.map((n) => env[n])].filter(Boolean);
  try {
    const opts = parseCommand(argv);
    const major = Number(process.versions.node.split('.')[0]);
    if (major < 20) throw new AscError(`Node ${process.versions.node}: this script needs Node 20 or later`, { usage: true });
    const store = ['prepare', 'submit'].includes(opts.command) || opts.dryRun ? readStore(opts.store, ['prepare', 'submit'].includes(opts.command) ? opts.version : undefined) : null;
    const waitMinutes = Number(env.ASC_WAIT_MINUTES ?? '60');
    if (!Number.isInteger(waitMinutes) || waitMinutes < 0 || waitMinutes > 60) throw new AscError('ASC_WAIT_MINUTES is 0 to 60', { usage: true });
    const creds = credentials(env);
    const ctx = { opts, store, env, log, summary, now, sleep, waitMinutes, dryRun: opts.dryRun };
    if (!creds) {
      if (opts.dryRun) return offlinePlan(ctx);
      throw new AscError('no credentials: set ASC_APP_ID, ASC_ISSUER_ID, ASC_KEY_ID and ASC_PRIVATE_KEY', { usage: true });
    }
    ctx.creds = creds;
    ctx.client = new Client({ creds, fetch, sleep, now, log, dryRun: opts.dryRun, secrets: secretsOf() });
    if (opts.dryRun) log('dry run: GET requests only; the writes are printed');
    const command = { status, 'check-new': checkNew, prepare: prepareCommand, submit, release }[opts.command];
    return await command(ctx);
  } catch (error) {
    if (!(error instanceof AscError)) {
      // no stack, no message: a library message could repeat a request header
      log(`::error::App Store Connect: unexpected ${error?.name ?? 'error'}`);
      return 1;
    }
    log(`::error::App Store Connect: ${mask(error.message, secretsOf())}`);
    if (error.hint) log(`::error::${error.hint}`);
    return error.usage ? 2 : 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  run(process.argv.slice(2)).then((code) => { process.exitCode = code; });
}
