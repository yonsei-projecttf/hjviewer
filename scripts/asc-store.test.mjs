// SPDX-License-Identifier: AGPL-3.0-only
// Offline tests of scripts/asc-store.mjs against a fake App Store Connect: no network, no Apple key
// (the tests sign with a throwaway P-256 key made here), made-up demo values only.
//   node --test scripts/asc-store.test.mjs   (scripts/ci.sh client)
import assert from 'node:assert/strict';
import { generateKeyPairSync, verify } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, cpSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createToken, fillNotes, notesBytes, readStore, run, STORE, NOTES_BYTES } from './asc-store.mjs';

const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const PEM = privateKey.export({ type: 'pkcs8', format: 'pem' });
const APP = '1234567890';
// made-up values; the tests check that none of them is ever printed
const REVIEW = {
  REVIEW_DEMO_USER: 'made-up-demo-user', REVIEW_DEMO_PASSWORD: 'made-up-demo-pass-77',
  REVIEW_DEMO_PC_ID: '987654321', REVIEW_DEMO_PC_PASSWORD: 'Made-up-PC-Password-42',
  REVIEW_CONTACT_FIRST_NAME: 'Testfirst', REVIEW_CONTACT_LAST_NAME: 'Testlast',
  REVIEW_CONTACT_PHONE: '+82-10-0000-0000', REVIEW_CONTACT_EMAIL: 'review-contact@example.test',
};
const KEY = { ASC_APP_ID: APP, ASC_ISSUER_ID: '00000000-1111-2222-3333-444444444444', ASC_KEY_ID: 'TESTKEY123', ASC_PRIVATE_KEY: PEM };
const ENV = { ...KEY, ...REVIEW, ASC_WAIT_MINUTES: '0' };
const SECRETS = [PEM.split('\n')[1], ...Object.values(REVIEW).filter((v) => v.length > 6)];

class FakeAsc {
  constructor() {
    this.n = 0;
    this.calls = [];
    this.lose = new Set(); // "METHOD path": apply the change, then lose the answer
    this.fail = new Map(); // "METHOD path" -> [status, body] answered once each
    this.builds = [{ id: 'b-192', build: '263120249', pre: '1.9.2', state: 'VALID', expired: false, expirationDate: '2027-01-20T00:00:00Z' }];
    this.versions = [];
    this.infos = [{ id: 'info-1', state: 'PREPARE_FOR_SUBMISSION', locs: [] }];
    this.submissions = [];
    this.primaryLocale = 'ko';
  }

  id(prefix) { this.n += 1; return `${prefix}-${this.n}`; }

  version(attrs) {
    const v = { id: this.id('v'), versionString: '1.0', appVersionState: 'PREPARE_FOR_SUBMISSION', releaseType: 'MANUAL', copyright: null, buildId: null, locs: [], detail: null, phased: null, ...attrs };
    this.versions.push(v);
    return v;
  }

  get writes() { return this.calls.filter(([m]) => m !== 'GET'); }

  fetch = async (url, options) => {
    const u = new URL(url);
    assert.equal(u.origin, 'https://api.appstoreconnect.apple.com');
    assert.equal(options.redirect, 'error');
    const [head, payload, signature] = options.headers.Authorization.replace(/^Bearer /, '').split('.');
    assert.ok(verify('sha256', Buffer.from(`${head}.${payload}`), { key: publicKey, dsaEncoding: 'ieee-p1363' }, Buffer.from(signature, 'base64url')));
    const body = options.body ? JSON.parse(options.body) : null;
    const key = `${options.method} ${u.pathname}`;
    this.calls.push([options.method, u.pathname, body, u.searchParams]);
    if (this.fail.has(key)) {
      const answers = this.fail.get(key);
      const [status, json] = answers.shift();
      if (!answers.length) this.fail.delete(key);
      return { status, ok: false, json: async () => json };
    }
    const json = this.route(options.method, u.pathname, u.searchParams, body);
    if (this.lose.has(key)) { this.lose.delete(key); throw new TypeError('fetch failed'); }
    if (json === 404) return { status: 404, ok: false, json: async () => ({ errors: [{ status: '404', code: 'NOT_FOUND' }] }) };
    return { status: json === null ? 204 : 200, ok: true, json: async () => json };
  };

  route(method, path, params, body) {
    const p = path.split('/').slice(2); // after /v1
    const verOf = (id) => this.versions.find((v) => v.id === id);
    const asVersion = (v) => ({ type: 'appStoreVersions', id: v.id, attributes: { versionString: v.versionString, appVersionState: v.appVersionState, releaseType: v.releaseType, copyright: v.copyright } });
    if (method === 'GET') {
      if (p[0] === 'builds') {
        assert.equal(params.get('filter[app]'), APP);
        const list = this.builds.filter((b) => (!params.get('filter[preReleaseVersion.version]') || b.pre === params.get('filter[preReleaseVersion.version]'))
          && (!params.get('filter[version]') || b.build === params.get('filter[version]')));
        return {
          data: list.map((b) => ({ type: 'builds', id: b.id, attributes: { version: b.build, processingState: b.state, expired: b.expired, expirationDate: b.expirationDate }, relationships: { preReleaseVersion: { data: { type: 'preReleaseVersions', id: `pre-${b.pre}` } } } })),
          included: [...new Set(list.map((b) => b.pre))].map((v) => ({ type: 'preReleaseVersions', id: `pre-${v}`, attributes: { version: v, platform: 'IOS' } })),
        };
      }
      if (p[0] === 'apps' && p.length === 2) {
        assert.equal(p[1], APP);
        return { data: { type: 'apps', id: APP, attributes: { primaryLocale: this.primaryLocale } } };
      }
      if (p[0] === 'apps' && p[2] === 'appStoreVersions') return { data: this.versions.map(asVersion), links: {} };
      if (p[0] === 'apps' && p[2] === 'reviewSubmissions') {
        const states = params.get('filter[state]').split(',');
        return { data: this.submissions.filter((s) => states.includes(s.state)).map((s) => ({ type: 'reviewSubmissions', id: s.id, attributes: { state: s.state, platform: 'IOS' } })) };
      }
      if (p[0] === 'reviewSubmissions' && p[2] === 'items') {
        const s = this.submissions.find((x) => x.id === p[1]);
        return { data: s.items.map((i) => ({ type: 'reviewSubmissionItems', id: i.id, relationships: { appStoreVersion: { data: i.versionId ? { type: 'appStoreVersions', id: i.versionId } : null } } })) };
      }
      if (p[0] === 'appStoreVersions') {
        const v = verOf(p[1]);
        if (p[2] === 'build') {
          const b = this.builds.find((x) => x.id === v.buildId);
          return { data: b ? { type: 'builds', id: b.id, attributes: { version: b.build } } : null };
        }
        if (p[2] === 'appStoreVersionLocalizations') return { data: v.locs.map((l) => ({ type: 'appStoreVersionLocalizations', id: l.id, attributes: l.attrs })) };
        if (p[2] === 'appStoreReviewDetail') return { data: v.detail ? { type: 'appStoreReviewDetails', id: v.detail.id, attributes: v.detail.attrs } : null };
        if (p[2] === 'appStoreVersionPhasedRelease') return v.phased ? { data: { type: 'appStoreVersionPhasedReleases', id: v.phased.id, attributes: { phasedReleaseState: v.phased.state } } } : 404;
      }
      if (p[0] === 'apps' && p[2] === 'appInfos') return { data: this.infos.map((i) => ({ type: 'appInfos', id: i.id, attributes: { state: i.state } })) };
      if (p[0] === 'appInfos' && p[2] === 'appInfoLocalizations') return { data: this.infos.find((i) => i.id === p[1]).locs.map((l) => ({ type: 'appInfoLocalizations', id: l.id, attributes: l.attrs })) };
      throw new Error(`unexpected GET ${path}`);
    }
    const d = body?.data;
    const rel = (name) => d.relationships[name].data.id;
    if (p[0] === 'appStoreVersions' && method === 'POST') {
      assert.equal(rel('app'), APP);
      return { data: asVersion(this.version({ versionString: d.attributes.versionString, releaseType: d.attributes.releaseType })) };
    }
    if (p[0] === 'appStoreVersions' && p[2] === 'relationships' && method === 'PATCH') { verOf(p[1]).buildId = d.id; return null; }
    if (p[0] === 'appStoreVersions' && method === 'PATCH') { Object.assign(verOf(p[1]), d.attributes); return { data: asVersion(verOf(p[1])) }; }
    if (p[0] === 'appStoreVersionLocalizations' || p[0] === 'appInfoLocalizations') {
      const owner = p[0] === 'appInfoLocalizations' ? this.infos.find((i) => i.id === (d.relationships ? rel('appInfo') : '')) : (d.relationships ? verOf(rel('appStoreVersion')) : null);
      if (method === 'POST') { owner.locs.push({ id: this.id('loc'), attrs: d.attributes }); return { data: { id: 'x' } }; }
      const all = [...this.versions.flatMap((v) => v.locs), ...this.infos.flatMap((i) => i.locs)];
      Object.assign(all.find((l) => l.id === p[1]).attrs, d.attributes);
      return { data: { id: p[1] } };
    }
    if (p[0] === 'appStoreReviewDetails') {
      if (method === 'POST') { verOf(rel('appStoreVersion')).detail = { id: this.id('rd'), attrs: { ...d.attributes } }; return { data: { id: 'rd' } }; }
      Object.assign(this.versions.find((v) => v.detail?.id === p[1]).detail.attrs, d.attributes);
      return { data: { id: p[1] } };
    }
    if (p[0] === 'appStoreVersionPhasedReleases') {
      if (method === 'POST') { verOf(rel('appStoreVersion')).phased = { id: this.id('ph'), state: d.attributes.phasedReleaseState }; return { data: { id: 'ph' } }; }
      this.versions.find((v) => v.phased?.id === p[1]).phased = null;
      return null;
    }
    if (p[0] === 'reviewSubmissions' && method === 'POST') {
      const s = { id: this.id('rs'), state: 'READY_FOR_REVIEW', items: [] };
      this.submissions.push(s);
      return { data: { type: 'reviewSubmissions', id: s.id } };
    }
    if (p[0] === 'reviewSubmissionItems') {
      this.submissions.find((s) => s.id === rel('reviewSubmission')).items.push({ id: this.id('item'), versionId: rel('appStoreVersion') });
      return { data: { id: 'item' } };
    }
    if (p[0] === 'reviewSubmissions' && method === 'PATCH') {
      assert.equal(d.attributes.submitted, true);
      const s = this.submissions.find((x) => x.id === p[1]);
      s.state = 'WAITING_FOR_REVIEW';
      for (const i of s.items) verOf(i.versionId).appVersionState = 'WAITING_FOR_REVIEW';
      return { data: { id: s.id } };
    }
    if (p[0] === 'appStoreVersionReleaseRequests') { verOf(rel('appStoreVersion')).appVersionState = 'PROCESSING_FOR_DISTRIBUTION'; return { data: { id: 'rr' } }; }
    throw new Error(`unexpected ${method} ${path}`);
  }
}

async function asc(fake, argv, env = ENV) {
  const lines = [];
  let clock = Date.parse('2026-10-28T00:00:00Z');
  const code = await run(argv, {
    env, fetch: fake ? fake.fetch : async () => { throw new Error('the script touched the network'); },
    sleep: async (ms) => { clock += ms; }, now: () => clock, out: (l) => lines.push(l),
  });
  const out = lines.join('\n');
  for (const secret of SECRETS) assert.ok(!out.includes(secret), `printed a secret value: ${secret.slice(0, 4)}…`);
  assert.ok(!/eyJ[A-Za-z0-9_-]{20,}\./.test(out), 'printed a token');
  return { code, out };
}

const PREPARE = ['prepare', '--version', '1.9.2', '--build', '263120249'];
const SUBMIT = ['submit', '--version', '1.9.2', '--build', '263120249'];

test('prepare on an app with no App Store version yet creates it and sends no What\'s New', async () => {
  const fake = new FakeAsc();
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 0, out);
  const [v] = fake.versions;
  assert.equal(v.versionString, '1.9.2');
  assert.equal(v.buildId, 'b-192');
  assert.equal(v.releaseType, 'MANUAL');
  assert.equal(v.copyright, readFileSync(join(STORE, 'copyright.txt'), 'utf8').trim());
  const store = readStore(STORE, '1.9.2');
  assert.deepEqual(v.locs.map((l) => l.attrs.locale), store.locales);
  for (const l of v.locs) {
    assert.equal('whatsNew' in l.attrs, false, 'the first App Store version got a What\'s New');
    assert.equal(l.attrs.description, store.texts[l.attrs.locale].version.description);
  }
  assert.deepEqual(fake.infos[0].locs.map((l) => l.attrs.name), store.locales.map((l) => store.texts[l].info.name));
  const detail = v.detail.attrs;
  assert.equal(detail.demoAccountName, REVIEW.REVIEW_DEMO_USER);
  assert.equal(detail.demoAccountRequired, true);
  assert.ok(detail.notes.includes(`ID ${REVIEW.REVIEW_DEMO_PC_ID}, password ${REVIEW.REVIEW_DEMO_PC_PASSWORD}`));
  assert.ok(!detail.notes.includes('{{'));
  assert.match(out, new RegExp(`notes \\(${notesBytes(detail.notes)} bytes filled\\)`));
  assert.equal(v.phased, null);
  assert.equal(fake.submissions.length, 0, 'prepare submitted');
  assert.match(out, /first App Store version: no What's New/);
});

test('prepare renumbers the editable version App Store Connect made with the app', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.0' });
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 0, out);
  assert.equal(fake.versions.length, 1);
  assert.equal(v.versionString, '1.9.2');
  assert.match(out, /renumber the editable version 1\.0/);
});

test('a version that is not editable stops prepare before any write', async () => {
  for (const state of ['WAITING_FOR_REVIEW', 'IN_REVIEW', 'PENDING_DEVELOPER_RELEASE', 'READY_FOR_DISTRIBUTION']) {
    const fake = new FakeAsc();
    fake.version({ versionString: '1.9.2', appVersionState: state });
    const { code, out } = await asc(fake, PREPARE);
    assert.equal(code, 1, out);
    assert.match(out, new RegExp(`1\\.9\\.2 is ${state}`));
    assert.deepEqual(fake.writes, []);
  }
  // another version still with App Review: no new version can be made
  const fake = new FakeAsc();
  fake.version({ versionString: '1.9.0', appVersionState: 'WAITING_FOR_REVIEW' });
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 1);
  assert.match(out, /1\.9\.0 is WAITING_FOR_REVIEW: App Store Connect takes no new version/);
  assert.deepEqual(fake.writes, []);
});

test('a later version gets What\'s New, the release type and the phased release', async () => {
  const fake = new FakeAsc();
  fake.version({ versionString: '1.9.2', appVersionState: 'READY_FOR_DISTRIBUTION' });
  fake.builds.push({ id: 'b-193', build: '263200000', pre: '1.9.3', state: 'VALID', expired: false, expirationDate: '2027-02-01T00:00:00Z' });
  const store = mkdtempSync(join(tmpdir(), 'hjv-asc-'));
  try {
    cpSync(STORE, store, { recursive: true });
    writeFileSync(join(store, 'version.txt'), '1.9.3\n');
    const { code, out } = await asc(fake, ['prepare', '--version', '1.9.3', '--build', '263200000', '--release-type', 'after-approval', '--phased', '--store', store]);
    assert.equal(code, 0, out);
  } finally { rmSync(store, { recursive: true, force: true }); }
  const v = fake.versions.find((x) => x.versionString === '1.9.3');
  assert.equal(v.releaseType, 'AFTER_APPROVAL');
  assert.ok(v.locs.every((l) => typeof l.attrs.whatsNew === 'string' && l.attrs.whatsNew.length > 0));
  assert.deepEqual(v.phased?.state, 'INACTIVE');
});

test('prepare again changes nothing', async () => {
  const fake = new FakeAsc();
  assert.equal((await asc(fake, PREPARE)).code, 0);
  const before = fake.writes.length;
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 0, out);
  assert.equal(fake.writes.length, before, fake.writes.slice(before).map(([m, p]) => `${m} ${p}`).join(', '));
  assert.match(out, /texts are store\/'s already/);
});

test('the store version must be the run\'s version', async () => {
  const { code, out } = await asc(new FakeAsc(), ['prepare', '--version', '1.9.9', '--build', '263120249']);
  assert.equal(code, 2);
  assert.match(out, /version\.txt is 1\.9\.2, the run's version is 1\.9\.9/);
});

test('prepare waits for the build\'s processing, and gives up', async () => {
  const fake = new FakeAsc();
  fake.builds[0].state = 'PROCESSING';
  let gets = 0;
  const original = fake.route.bind(fake);
  fake.route = (method, path, params, body) => {
    if (method === 'GET' && path === '/v1/builds' && ++gets === 3) fake.builds[0].state = 'VALID';
    return original(method, path, params, body);
  };
  const { code, out } = await asc(fake, PREPARE, { ...ENV, ASC_WAIT_MINUTES: '60' });
  assert.equal(code, 0, out);
  assert.match(out, /PROCESSING; waiting/);
  const none = new FakeAsc();
  none.builds = [];
  const r = await asc(none, PREPARE, { ...ENV, ASC_WAIT_MINUTES: '2' });
  assert.equal(r.code, 1);
  assert.match(r.out, /has no iOS 1\.9\.2 build 263120249/);
  assert.match(r.out, /waited 2 minutes/);
  const failed = new FakeAsc();
  failed.builds[0].state = 'INVALID';
  assert.match((await asc(failed, PREPARE)).out, /is INVALID/);
  assert.deepEqual(failed.writes, []);
});

test('a missing review secret stops prepare before any call', async () => {
  const env = { ...ENV };
  delete env.REVIEW_DEMO_PC_ID;
  const fake = new FakeAsc();
  const { code, out } = await asc(fake, PREPARE, env);
  assert.equal(code, 1);
  assert.match(out, /REVIEW_DEMO_PC_ID is not set/);
  assert.deepEqual(fake.calls, []);
});

test('submit without an open submission makes one and submits the version', async () => {
  const fake = new FakeAsc();
  const { code, out } = await asc(fake, SUBMIT);
  assert.equal(code, 0, out);
  assert.equal(fake.submissions.length, 1);
  const [s] = fake.submissions;
  assert.equal(s.state, 'WAITING_FOR_REVIEW');
  assert.deepEqual(s.items.map((i) => i.versionId), [fake.versions[0].id]);
  assert.match(out, /Submitted App Store version 1\.9\.2 \(build 263120249\) for App Review \(manual release\)/);
  assert.match(out, /unlisted app request/);
  // run again (an answer lost, a re-run): nothing is written
  const before = fake.writes.length;
  const again = await asc(fake, SUBMIT);
  assert.equal(again.code, 0, again.out);
  assert.match(again.out, /submitted already; nothing to do/);
  assert.equal(fake.writes.length, before);
});

test('submit uses the READY_FOR_REVIEW submission again and adds the missing item', async () => {
  const fake = new FakeAsc();
  fake.submissions.push({ id: 'rs-open', state: 'READY_FOR_REVIEW', items: [] });
  const { code, out } = await asc(fake, SUBMIT);
  assert.equal(code, 0, out);
  assert.equal(fake.submissions.length, 1);
  assert.equal(fake.submissions[0].state, 'WAITING_FOR_REVIEW');
  assert.equal(fake.submissions[0].items.length, 1);
  assert.equal(fake.writes.filter(([m, p]) => m === 'POST' && p === '/v1/reviewSubmissions').length, 0);
});

test('submit after an unanswered submitted=true ends as success', async () => {
  const fake = new FakeAsc();
  const original = fake.route.bind(fake);
  fake.route = (method, path, params, body) => {
    const answer = original(method, path, params, body);
    if (method === 'PATCH' && path.startsWith('/v1/reviewSubmissions/')) fake.lose.add(`${method} ${path}`);
    return answer;
  };
  const first = await asc(fake, SUBMIT);
  assert.equal(first.code, 1);
  assert.match(first.out, /did not answer PATCH \/v1\/reviewSubmissions/);
  assert.match(first.out, /run the same command again/);
  fake.route = original;
  const before = fake.writes.length;
  const again = await asc(fake, SUBMIT);
  assert.equal(again.code, 0, again.out);
  assert.match(again.out, /WAITING_FOR_REVIEW: submitted already/);
  assert.equal(fake.writes.length, before);
});

test('an UNRESOLVED_ISSUES submission of this version is fixed and sent again', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.9.2', appVersionState: 'METADATA_REJECTED', buildId: 'b-192' });
  fake.submissions.push({ id: 'rs-old', state: 'UNRESOLVED_ISSUES', items: [{ id: 'item-old', versionId: v.id }] });
  const { code, out } = await asc(fake, SUBMIT);
  assert.equal(code, 0, out);
  assert.equal(fake.submissions.length, 1);
  assert.equal(fake.submissions[0].state, 'WAITING_FOR_REVIEW');
  assert.deepEqual(fake.writes.filter(([m, p]) => p.startsWith('/v1/reviewSubmission')).map(([m, p]) => `${m} ${p}`), ['PATCH /v1/reviewSubmissions/rs-old']);
});

test('an open submission with other items stops submit before any write', async () => {
  const fake = new FakeAsc();
  const other = fake.version({ versionString: '1.9.0', appVersionState: 'READY_FOR_DISTRIBUTION' });
  fake.submissions.push({ id: 'rs-other', state: 'READY_FOR_REVIEW', items: [{ id: 'i1', versionId: other.id }, { id: 'i2', versionId: null }] });
  const { code, out } = await asc(fake, SUBMIT);
  assert.equal(code, 1);
  assert.match(out, /holds 2 other item\(s\)/);
  assert.match(out, /cancels nothing/);
  assert.deepEqual(fake.writes, []);
});

test('release is done once and then reports success', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.9.2', appVersionState: 'PENDING_DEVELOPER_RELEASE', buildId: 'b-192' });
  const wrong = await asc(fake, ['release', '--version', '1.9.2', '--build', '1']);
  assert.equal(wrong.code, 1);
  assert.match(wrong.out, /has build 263120249, not 1/);
  const { code, out } = await asc(fake, ['release', '--version', '1.9.2', '--build', '263120249']);
  assert.equal(code, 0, out);
  assert.equal(v.appVersionState, 'PROCESSING_FOR_DISTRIBUTION');
  const again = await asc(fake, ['release', '--version', '1.9.2']);
  assert.equal(again.code, 0);
  assert.match(again.out, /released already; nothing to do/);
  assert.equal(fake.writes.length, 1);
  v.appVersionState = 'WAITING_FOR_REVIEW';
  const early = await asc(fake, ['release', '--version', '1.9.2']);
  assert.equal(early.code, 1);
  assert.match(early.out, /only an approved version of manual release/);
});

test('check-new fails when the version has a build already', async () => {
  const fake = new FakeAsc();
  const r = await asc(fake, ['check-new', '--version', '1.9.2'], KEY);
  assert.equal(r.code, 1);
  assert.match(r.out, /has iOS 1\.9\.2 already \(build 263120249, VALID\)/);
  assert.match(r.out, /raise client\/version\.env/);
  const ok = await asc(fake, ['check-new', '--version', '1.9.3'], KEY);
  assert.equal(ok.code, 0, ok.out);
  assert.ok(fake.calls.every(([m]) => m === 'GET'));
});

test('status gives one line and a JSON file, with GET requests only', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.9.2', appVersionState: 'WAITING_FOR_REVIEW', buildId: 'b-192' });
  fake.version({ versionString: '1.9.0', appVersionState: 'READY_FOR_DISTRIBUTION' });
  v.phased = { id: 'ph-1', state: 'INACTIVE' };
  fake.submissions.push({ id: 'rs-1', state: 'WAITING_FOR_REVIEW', items: [{ id: 'i', versionId: v.id }] });
  const dir = mkdtempSync(join(tmpdir(), 'hjv-asc-'));
  try {
    const file = join(dir, 'status.json');
    const { code, out } = await asc(fake, ['status', '--version', '1.9.2', '--json', file], KEY);
    assert.equal(code, 0, out);
    assert.match(out, /App Store: on the App Store 1\.9\.0; in review 1\.9\.2; 1\.9\.2 WAITING_FOR_REVIEW, phased INACTIVE; TestFlight 1\.9\.2 \(263120249\) VALID/);
    const report = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(report.in_review, ['1.9.2']);
    assert.equal(report.on_app_store, '1.9.0');
    assert.deepEqual(report.open_submissions, [{ id: 'rs-1', state: 'WAITING_FOR_REVIEW', versions: ['1.9.2'] }]);
  } finally { rmSync(dir, { recursive: true, force: true }); }
  assert.deepEqual(fake.writes, []);
  // without a version, and with no phased release (404)
  const r = await asc(fake, ['status'], KEY);
  assert.equal(r.code, 0, r.out);
});

test('a dry run with the key reads only and prints the writes without secret values', async () => {
  const fake = new FakeAsc();
  fake.version({ versionString: '1.0' });
  const { code, out } = await asc(fake, [...SUBMIT, '--dry-run']);
  assert.equal(code, 0, out);
  assert.ok(fake.calls.every(([m]) => m === 'GET'), fake.writes.map(([m, p]) => `${m} ${p}`).join(', '));
  assert.match(out, /would PATCH \/v1\/appStoreVersions\/v-1: renumber/);
  assert.match(out, /would POST \/v1\/appStoreReviewDetails: App Review information: contact, demo account \(required\), notes \(\d+ bytes filled\)/);
  assert.match(out, /would PATCH \/v1\/reviewSubmissions\/<new submission>: submit for App Review/);
  assert.match(out, /Dry run: nothing was changed or submitted/);
});

test('a dry run without the key needs no network', async () => {
  for (const argv of [[...PREPARE, '--dry-run'], [...SUBMIT, '--dry-run', '--phased'], ['release', '--version', '1.9.2', '--dry-run'],
    ['check-new', '--version', '1.9.2', '--dry-run'], ['status', '--dry-run']]) {
    const { code, out } = await asc(null, argv, {});
    assert.equal(code, 0, out);
    assert.match(out, /no network/);
  }
  const { out } = await asc(null, [...PREPARE, '--dry-run'], {});
  assert.match(out, /notes from review_notes\.txt \(\d+ bytes before filling\)/);
  assert.match(out, /0 of 8 set here/);
});

test('the filled notes keep check-store-text.py\'s limits and never show the values', () => {
  const template = readStore(STORE, '1.9.2').notes;
  const filled = fillNotes(template, REVIEW);
  assert.ok(filled.bytes <= NOTES_BYTES);
  for (const [env, why] of [
    [{ ...REVIEW, REVIEW_DEMO_PC_ID: '1'.repeat(17) }, /REVIEW_DEMO_PC_ID is longer than 16 bytes/],
    [{ ...REVIEW, REVIEW_DEMO_PC_PASSWORD: 'p'.repeat(33) }, /REVIEW_DEMO_PC_PASSWORD is longer than 32 bytes/],
    [{ ...REVIEW, REVIEW_DEMO_PC_PASSWORD: 'with space' }, /printable ASCII/],
    [{ ...REVIEW, REVIEW_DEMO_PC_PASSWORD: '비밀번호비밀번호' }, /printable ASCII/],
    [{ ...REVIEW, REVIEW_DEMO_PC_ID: '' }, /REVIEW_DEMO_PC_ID is not set/],
  ]) {
    assert.throws(() => fillNotes(template, env), (e) => why.test(e.message) && !e.message.includes(env.REVIEW_DEMO_PC_PASSWORD));
  }
  // the placeholders once each, nothing left to fill, 4000 bytes with line breaks as CRLF
  assert.throws(() => fillNotes(`${template}\n{{DEMO_PC_ID}}`, REVIEW), /must hold \{\{DEMO_PC_ID\}\} once/);
  assert.throws(() => fillNotes(`${template}\n{{OTHER}}`, REVIEW), /cannot fill/);
  const long = `{{DEMO_PC_ID}} {{DEMO_PC_PASSWORD}}\n${'x\n'.repeat(1990)}`;
  assert.throws(() => fillNotes(long, REVIEW), /bytes; App Store Connect takes 4000/);
  assert.equal(notesBytes('a\nb'), 4);
});

test('Apple\'s errors are shown with the secret values masked, and GET requests are retried', async () => {
  const fake = new FakeAsc();
  fake.fail.set('GET /v1/apps/1234567890/appStoreVersions', [[503, {}], [503, {}]]);
  fake.fail.set('POST /v1/appStoreReviewDetails', [[409, { errors: [{ code: 'ENTITY_ERROR.ATTRIBUTE.INVALID', title: 'invalid', detail: `the value ${REVIEW.REVIEW_DEMO_PASSWORD} is not allowed` }] }]]);
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 1);
  assert.match(out, /HTTP 409: ENTITY_ERROR\.ATTRIBUTE\.INVALID invalid: the value <masked> is not allowed/);
  assert.equal(fake.calls.filter(([m, p]) => p.endsWith('/appStoreVersions') && m === 'GET').length, 3);
  const denied = new FakeAsc();
  denied.fail.set('GET /v1/builds', [[401, { errors: [{ code: 'NOT_AUTHORIZED' }] }]]);
  const r = await asc(denied, ['check-new', '--version', '1.9.3'], KEY);
  assert.equal(r.code, 1);
  assert.match(r.out, /a revoked key answers 401/);
});

test('bad usage and bad keys exit 2', async () => {
  for (const [argv, env, why] of [
    [['publish'], KEY, /the command is one of/],
    [['prepare', '--version', '1.9.2'], KEY, /needs --build/],
    [['submit', '--version', '1.9', '--build', '1'], KEY, /not a version/],
    [['release'], KEY, /needs --version/],
    [[...PREPARE, '--release-type', 'scheduled'], KEY, /manual or after-approval/],
    [['status'], { ...KEY, ASC_PRIVATE_KEY: 'not a key' }, /not a private key/],
    [['status'], { ASC_APP_ID: APP }, /together/],
    [['status'], { ...KEY, ASC_APP_ID: 'app' }, /numeric Apple ID/],
  ]) {
    const { code, out } = await asc(new FakeAsc(), argv, env);
    assert.equal(code, 2, `${argv.join(' ')}: ${out}`);
    assert.match(out, why);
  }
});

test('the first App Store version takes neither after-approval nor a phased release', async () => {
  for (const extra of [['--release-type', 'after-approval'], ['--phased'], ['--release-type', 'after-approval', '--phased']]) {
    for (const argv of [[...PREPARE, ...extra], [...SUBMIT, ...extra], [...SUBMIT, ...extra, '--dry-run']]) {
      const fake = new FakeAsc();
      fake.version({ versionString: '1.0' });
      const { code, out } = await asc(fake, argv);
      assert.equal(code, 2, `${argv.join(' ')}: ${out}`);
      assert.match(out, extra[0] === '--phased' ? /from the second App Store version on/ : /released by hand: --release-type manual/);
      assert.deepEqual(fake.writes, [], argv.join(' '));
      assert.equal(fake.versions[0].versionString, '1.0');
    }
  }
});

test('after a binary rejection submit takes the new version through the rejected one', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.9.2', appVersionState: 'REJECTED', buildId: 'b-192' });
  fake.submissions.push({ id: 'rs-old', state: 'UNRESOLVED_ISSUES', items: [{ id: 'item-old', versionId: v.id }] });
  fake.builds.push({ id: 'b-193', build: '263200000', pre: '1.9.3', state: 'VALID', expired: false, expirationDate: '2027-02-01T00:00:00Z' });
  const store = mkdtempSync(join(tmpdir(), 'hjv-asc-'));
  try {
    cpSync(STORE, store, { recursive: true });
    writeFileSync(join(store, 'version.txt'), '1.9.3\n');
    const { code, out } = await asc(fake, ['submit', '--version', '1.9.3', '--build', '263200000', '--store', store]);
    assert.equal(code, 0, out);
  } finally { rmSync(store, { recursive: true, force: true }); }
  assert.equal(fake.versions.length, 1);
  assert.equal(v.versionString, '1.9.3');
  assert.equal(v.buildId, 'b-193');
  assert.equal(fake.submissions.length, 1);
  assert.equal(fake.submissions[0].state, 'WAITING_FOR_REVIEW');
  assert.equal(fake.submissions[0].items.length, 1);
});

test('submit and release that are done already must hold the build of the run', async () => {
  const fake = new FakeAsc();
  const v = fake.version({ versionString: '1.9.2', appVersionState: 'WAITING_FOR_REVIEW', buildId: 'b-192' });
  const wrong = await asc(fake, ['submit', '--version', '1.9.2', '--build', '999']);
  assert.equal(wrong.code, 1, wrong.out);
  assert.match(wrong.out, /has build 263120249, not 999/);
  v.appVersionState = 'PREPARE_FOR_SUBMISSION';
  fake.submissions.push({ id: 'rs-1', state: 'WAITING_FOR_REVIEW', items: [{ id: 'i', versionId: v.id }] });
  const lagging = await asc(fake, ['submit', '--version', '1.9.2', '--build', '999']);
  assert.equal(lagging.code, 1, lagging.out);
  assert.match(lagging.out, /has build 263120249, not 999/);
  for (const state of ['READY_FOR_DISTRIBUTION', 'PROCESSING_FOR_DISTRIBUTION']) {
    v.appVersionState = state;
    const r = await asc(fake, ['release', '--version', '1.9.2', '--build', '999']);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /has build 263120249, not 999/);
    const ok = await asc(fake, ['release', '--version', '1.9.2', '--build', '263120249']);
    assert.equal(ok.code, 0, ok.out);
    assert.match(ok.out, /released already/);
  }
  assert.deepEqual(fake.writes, []);
});

test('a primary language other than the first line of locales.txt stops prepare before any write', async () => {
  const fake = new FakeAsc();
  fake.primaryLocale = 'en-US';
  const { code, out } = await asc(fake, PREPARE);
  assert.equal(code, 1, out);
  assert.match(out, /primary language in App Store Connect is en-US, store\/appstore\/locales\.txt starts with ko/);
  assert.deepEqual(fake.writes, []);
});

test('a bad input value cannot start a workflow command line', async () => {
  const { code, out } = await asc(new FakeAsc(), ['status', '--version', '1.0\n::warning::injected'], KEY);
  assert.equal(code, 2);
  assert.ok(out.split('\n').every((l) => !l.startsWith('::warning::')), out);
  for (const v of ['1.9.100', '1000.0.0', '01.9.1']) {
    assert.equal((await asc(new FakeAsc(), ['status', '--version', v], KEY)).code, 2, v);
  }
});

test('the token is ES256 for App Store Connect, at most 20 minutes', () => {
  const token = createToken({ keyId: 'TESTKEY123', issuerId: KEY.ASC_ISSUER_ID, privateKey: PEM }, 1000);
  const [head, payload, signature] = token.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(head, 'base64url')), { alg: 'ES256', kid: 'TESTKEY123', typ: 'JWT' });
  const claims = JSON.parse(Buffer.from(payload, 'base64url'));
  assert.equal(claims.aud, 'appstoreconnect-v1');
  assert.ok(claims.exp - claims.iat <= 1200);
  assert.equal(Buffer.from(signature, 'base64url').length, 64);
});
