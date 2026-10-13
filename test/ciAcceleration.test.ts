import { expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  changedFiles,
  classify,
  classifyChangedFiles,
  envWithoutGitBindings,
} from '../scripts/classify-ci-changes.mjs';
import {
  PREPARED_FLAG,
  missingPreparedResources,
  prepareTauriBuild,
  preparedResources,
  shouldSkipPreparation,
} from '../scripts/prepare-tauri-build.mjs';
import { rustSidecarBuildPlan } from '../scripts/build-rust-sidecar.mjs';

const workflow = readFileSync(
  new URL('../.github/workflows/build.yaml', import.meta.url),
  'utf8'
);
const tauriConfig = JSON.parse(
  readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')
);

const PLATFORM_JOBS = [
  ['  build-tauri-arm64:', '  build-tauri-windows-x64:'],
  ['  build-tauri-windows-x64:', '  build-tauri-linux-x64:'],
  ['  build-tauri-linux-x64:', '  draft-release:'],
] as const;

function jobBody(start: string, end: string): string {
  return workflow.slice(workflow.indexOf(start), workflow.indexOf(end));
}

async function prepareFixture(
  targetTriple = 'aarch64-apple-darwin'
): Promise<string> {
  const root = await mkdtemp(path.join(tmpdir(), 'ypm-prepared-'));
  const sidecarName = rustSidecarBuildPlan({ targetTriple }).outputName;
  const files = [
    path.join(root, 'dist-tauri', 'index.html'),
    path.join(root, 'dist-tauri', 'taskbar-overlay.html'),
    path.join(root, 'src-tauri/generated/app-compliance/SHA256SUMS'),
    path.join(
      root,
      'src-tauri/generated/app-compliance/APP-COMPLIANCE-MANIFEST.json'
    ),
    path.join(root, 'src-tauri', 'binaries', sidecarName),
    path.join(root, 'src-tauri/generated/sidecar-compliance/SHA256SUMS'),
    path.join(
      root,
      'src-tauri/generated/sidecar-compliance/SOURCE-MANIFEST.json'
    ),
    path.join(root, 'src-tauri/generated/sidecar-complete-source/SHA256SUMS'),
    path.join(
      root,
      'src-tauri/generated/sidecar-complete-source/SOURCE-MANIFEST.json'
    ),
    path.join(
      root,
      'src-tauri/generated/sidecar-complete-source/.cargo/config.toml'
    ),
  ];
  for (const file of files) {
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, 'x');
  }
  for (const directory of [
    path.join(root, 'dist-tauri', 'assets'),
    path.join(
      root,
      'src-tauri/generated/sidecar-complete-source/source/vendor'
    ),
  ]) {
    await mkdir(directory, { recursive: true });
    await writeFile(path.join(directory, 'asset'), 'x');
  }
  return root;
}

test('七条打包路径统一走同一个 beforeBuild wrapper', () => {
  expect(tauriConfig.build.beforeBuildCommand).toBe(
    'bun scripts/prepare-tauri-build.mjs'
  );
  const packagingScripts = JSON.parse(
    readFileSync(new URL('../package.json', import.meta.url), 'utf8')
  ).scripts;
  expect(packagingScripts).toMatchObject({
    'build:tauri': 'bun scripts/build-tauri-host.mjs',
    'build:tauri:macos:updater':
      'bun scripts/build-tauri-updater.mjs darwin-aarch64',
    'build:tauri:release':
      'bun scripts/build-tauri-updater.mjs darwin-aarch64 --developer-id',
    'build:tauri:windows':
      'tauri build --target x86_64-pc-windows-msvc --bundles nsis --ci',
    'build:tauri:windows:updater':
      'bun scripts/build-tauri-updater.mjs windows-x86_64',
    'build:tauri:linux':
      'tauri build --verbose --target x86_64-unknown-linux-gnu --bundles deb,appimage --ci',
    'build:tauri:linux:updater':
      'bun scripts/build-tauri-updater.mjs linux-x86_64',
  });
});

test('没有 flag 时生产入口按顺序执行两项准备工作', () => {
  expect(shouldSkipPreparation({})).toBe(false);
  expect(shouldSkipPreparation({ [PREPARED_FLAG]: '1' })).toBe(false);
  expect(
    shouldSkipPreparation({ GITHUB_ACTIONS: 'true', [PREPARED_FLAG]: '0' })
  ).toBe(false);
  const commands: string[][] = [];
  expect(
    prepareTauriBuild({
      env: {},
      runCommand: command => commands.push(command),
      log: () => {},
    })
  ).toEqual({ skipped: false });
  expect(commands).toEqual([
    ['run', 'build:tauri:renderer'],
    ['run', 'build:sidecar'],
  ]);
});

test('只有 CI 里设了 flag 才跳过', () => {
  expect(
    shouldSkipPreparation({ GITHUB_ACTIONS: 'true', [PREPARED_FLAG]: '1' })
  ).toBe(true);
});

test('产物齐全才认，缺任何一类都报出缺什么', async () => {
  const root = await prepareFixture();
  try {
    expect(
      missingPreparedResources({ root, targetTriple: 'aarch64-apple-darwin' })
    ).toEqual([]);

    await rm(path.join(root, 'src-tauri', 'binaries'), {
      recursive: true,
      force: true,
    });
    const missing = missingPreparedResources({
      root,
      targetTriple: 'aarch64-apple-darwin',
    });
    expect(missing).toHaveLength(1);
    expect(missing[0]?.label).toBe('Rust Sidecar');
    expect(missing[0]?.absent[0]).toContain('yesplaymusic-sidecar');
    expect(() =>
      prepareTauriBuild({
        env: { GITHUB_ACTIONS: 'true', [PREPARED_FLAG]: '1' },
        root,
        targetTriple: 'aarch64-apple-darwin',
        runCommand: () => {
          throw new Error('不应执行 producer');
        },
        log: () => {},
      })
    ).toThrow('Rust Sidecar');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('三平台准备门禁检查 producer 实际生成的 Sidecar 文件名', () => {
  for (const targetTriple of [
    'aarch64-apple-darwin',
    'x86_64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu',
  ]) {
    const sidecar = preparedResources({
      root: '/repo',
      targetTriple,
    }).find(resource => resource.label === 'Rust Sidecar');
    expect(sidecar?.files).toHaveLength(1);
    expect(path.basename(sidecar?.files[0] ?? '')).toBe(
      rustSidecarBuildPlan({ targetTriple }).outputName
    );
  }
  expect(
    rustSidecarBuildPlan({ targetTriple: 'x86_64-pc-windows-msvc' }).outputName
  ).toEndWith('.exe');
});

test('flag 下产物齐全时生产入口零构建', async () => {
  const targetTriple = 'x86_64-pc-windows-msvc';
  const root = await prepareFixture(targetTriple);
  const commands: string[][] = [];
  try {
    expect(
      prepareTauriBuild({
        env: { GITHUB_ACTIONS: 'true', [PREPARED_FLAG]: '1' },
        root,
        targetTriple,
        runCommand: command => commands.push(command),
        log: () => {},
      })
    ).toEqual({ skipped: true });
    expect(commands).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('完整 Sidecar 源码套件缺失时不允许跳过', async () => {
  const root = await prepareFixture();
  try {
    await rm(
      path.join(
        root,
        'src-tauri/generated/sidecar-complete-source/source/vendor'
      ),
      { recursive: true, force: true }
    );
    const missing = missingPreparedResources({
      root,
      targetTriple: 'aarch64-apple-darwin',
    });
    expect(missing.map(resource => resource.label)).toEqual([
      'complete Sidecar source',
    ]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('空文件不算数，避免半截产物骗过检查', async () => {
  const root = await prepareFixture();
  try {
    await writeFile(path.join(root, 'dist-tauri', 'index.html'), '', 'utf8');
    const missing = missingPreparedResources({
      root,
      targetTriple: 'aarch64-apple-darwin',
    });
    expect(missing.map(resource => resource.label)).toEqual(['renderer']);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('三平台 job 都先准备资源、再打标记，顺序不能反', () => {
  for (const [start, end] of PLATFORM_JOBS) {
    const job = jobBody(start, end);
    const prepareIndex = job.indexOf('bun run build:tauri:renderer');
    const markIndex = job.indexOf(`echo "${PREPARED_FLAG}=1"`);
    const buildIndex = job.indexOf('run: bun run build:tauri');
    expect(prepareIndex).toBeGreaterThan(-1);
    expect(markIndex).toBeGreaterThan(prepareIndex);
    expect(buildIndex).toBeGreaterThan(markIndex);
  }
});

test('三处 rust-cache 都不再 cache-on-failure', () => {
  expect(workflow).not.toContain('cache-on-failure');
  expect(workflow.match(/Swatinem\/rust-cache@v2/g)).toHaveLength(4);
});

test('纯文档改动跳打包，但门禁照跑', () => {
  expect(classifyChangedFiles(['README.md', 'docs/a.md'])).toEqual({
    docsOnly: true,
    rust: false,
    tuiOnly: false,
  });
  expect(classifyChangedFiles(['images/screenshots/player.png'])).toEqual({
    docsOnly: true,
    rust: false,
    tuiOnly: false,
  });
  expect(workflow).toContain('docs-gates');
  const docsJob = jobBody('  docs-gates:', '  build-tauri-arm64:');
  expect(docsJob).toContain("if: needs.changes.outputs.docs-only == 'true'");
  expect(docsJob).toContain('run: bun test');
  expect(docsJob).toContain('run: bun run typecheck');
  for (const [start, end] of PLATFORM_JOBS) {
    expect(jobBody(start, end)).toContain(
      "if: needs.changes.outputs.docs-only != 'true'"
    );
  }
});

test('README 断言所在的测试文件跟着 README 走，其余 test/ 不算文档', () => {
  expect(
    classifyChangedFiles([
      'README.md',
      'images/tui-library.png',
      'test/tauriRelease.test.ts',
    ])
  ).toEqual({ docsOnly: true, rust: false, tuiOnly: false });
  expect(classifyChangedFiles(['test/appUpdater.test.ts'])).toEqual({
    docsOnly: false,
    rust: false,
    tuiOnly: false,
  });
  // Rust integration tests read this fixture, so it must keep the Rust gates.
  expect(classifyChangedFiles(['test/fixtures/proxy-ca.pem'])).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
});

test('嵌入 ypm 的 logo 改动触发 TUI 构建', () => {
  expect(classifyChangedFiles(['images/logo.png'])).toEqual({
    docsOnly: false,
    rust: false,
    tuiOnly: true,
  });
});

test('TUI、文档与桌面 Rust 路径按三分类矩阵分流', () => {
  const cases = [
    {
      files: ['src-tauri/tui/src/main.rs'],
      expected: { docsOnly: false, rust: false, tuiOnly: true },
    },
    {
      files: ['src-tauri/tui/Cargo.toml', 'docs/tui.md'],
      expected: { docsOnly: false, rust: false, tuiOnly: true },
    },
    {
      files: ['src-tauri/tui/src/main.rs', 'src-tauri/core/src/lib.rs'],
      expected: { docsOnly: false, rust: true, tuiOnly: false },
    },
    {
      files: ['README.md', 'docs/a.md'],
      expected: { docsOnly: true, rust: false, tuiOnly: false },
    },
    {
      files: ['src-tauri/core/src/lib.rs'],
      expected: { docsOnly: false, rust: true, tuiOnly: false },
    },
  ];

  for (const { files, expected } of cases) {
    expect(classifyChangedFiles(files)).toEqual(expected);
  }
});

test('合规文本是打包输入，不能按纯文档跳过产物验证', () => {
  for (const file of [
    'LICENSE',
    'legal/GPL-3.0.txt',
    'legal/app-license-donors/objc2/LICENSE.md',
  ]) {
    expect(classifyChangedFiles([file])).toEqual({
      docsOnly: false,
      rust: false,
      tuiOnly: false,
    });
  }
});

test('只改渲染层时跳过 Rust 门禁，改 Rust 时全跑', () => {
  expect(classifyChangedFiles(['src/App.vue', 'src/utils/x.ts'])).toEqual({
    docsOnly: false,
    rust: false,
    tuiOnly: false,
  });
  expect(classifyChangedFiles(['src-tauri/src/main.rs'])).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
  expect(classifyChangedFiles(['scripts/build-tauri-host.mjs']).rust).toBe(
    true
  );
  expect(classifyChangedFiles(['.github/workflows/build.yaml']).rust).toBe(
    true
  );
  expect(classifyChangedFiles(['src/sidecar-route-manifest.json']).rust).toBe(
    true
  );
  expect(classifyChangedFiles(['test/fixtures/proxy-ca.pem']).rust).toBe(true);
  expect(
    classifyChangedFiles([
      'src-tauri/sidecar/src/fixtures/audio-tags/README.md',
    ])
  ).toEqual({ docsOnly: false, rust: true, tuiOnly: false });
  expect(classifyChangedFiles(['.cargo/config.toml'])).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });

  for (const [start, end] of PLATFORM_JOBS) {
    const job = jobBody(start, end);
    expect(
      job.match(/if: needs\.changes\.outputs\.rust == 'true'/g)
    ).toHaveLength(5);
    // Preparing resources is packaging work, not a Rust gate: never skipped.
    const prepareBlock = job.slice(
      job.indexOf('- name: Prepare Tauri resources for Rust tests'),
      job.indexOf('- name: Mark Tauri resources as prepared')
    );
    expect(prepareBlock).not.toContain('needs.changes.outputs.rust');
  }
});

test('分不清改动范围时一律全跑', () => {
  expect(classifyChangedFiles([])).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
  expect(classify({ FORCE_FULL: 'true' })).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
  // Force push and first push give an all-zero or missing base.
  expect(classify({ BASE_SHA: '0'.repeat(40), HEAD_SHA: 'abc' })).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
  expect(classify({ HEAD_SHA: 'abc' })).toEqual({
    docsOnly: false,
    rust: true,
    tuiOnly: false,
  });
});

test('tag、手动触发与 force push 强制全跑', () => {
  const classifyJob = jobBody('  changes:', '  docs-gates:');
  expect(classifyJob).toContain("github.ref_type == 'tag'");
  expect(classifyJob).toContain("github.event_name == 'workflow_dispatch'");
  expect(classifyJob).toContain('github.event.forced == true');
  expect(classifyJob).toContain('fetch-depth: 0');
});

test('rename 同时分类旧路径和新路径，不能把 Rust 删除伪装成文档改动', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ypm-ci-rename-'));
  // Explicit env, or a pre-commit hook's GIT_DIR makes this fixture git
  // (and mutating commands like `git commit`) target the real repository.
  // Bun's delete process.env does not reach child processes, so the env
  // object must be passed to spawnSync directly.
  const gitEnv = envWithoutGitBindings();
  const git = (args: string[]): string => {
    const result = spawnSync('git', args, {
      cwd: root,
      encoding: 'utf8',
      env: gitEnv,
    });
    if (result.status !== 0) throw new Error(result.stderr);
    return result.stdout.trim();
  };
  try {
    git(['init', '--quiet']);
    git(['config', 'user.name', 'CI test']);
    git(['config', 'user.email', 'ci@example.invalid']);
    await mkdir(path.join(root, 'src-tauri', 'src'), { recursive: true });
    await writeFile(path.join(root, 'src-tauri', 'src', 'retired.rs'), 'old');
    git(['add', '.']);
    git(['commit', '--quiet', '-m', 'base']);
    const baseSha = git(['rev-parse', 'HEAD']);
    await mkdir(path.join(root, 'docs'), { recursive: true });
    git(['mv', 'src-tauri/src/retired.rs', 'docs/retired.md']);
    git(['commit', '--quiet', '-m', 'rename']);
    const headSha = git(['rev-parse', 'HEAD']);

    const files = changedFiles({ baseSha, headSha, cwd: root });
    expect(files?.sort()).toEqual([
      'docs/retired.md',
      'src-tauri/src/retired.rs',
    ]);
    expect(classifyChangedFiles(files ?? [])).toEqual({
      docsOnly: false,
      rust: true,
      tuiOnly: false,
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
