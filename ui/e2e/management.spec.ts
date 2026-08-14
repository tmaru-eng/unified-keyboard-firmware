import { expect, test } from '@playwright/test';

test.describe('管理WebUIのシミュレーション総合経路', () => {
  test('未接続のデモからシミュレーション出力を診断できる', async ({ page }) => {
    await page.goto('/');

    await page.getByRole('button', { name: 'デモ', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'ブリッジ動作デモ' })).toBeVisible();
    await page.getByRole('button', { name: 'USB: Shift + 2 (@)' }).click();
    await page.getByRole('button', { name: '診断', exact: true }).click();
    await expect(page.getByText('SIMULATED OUTPUT')).toBeVisible();
  });

  test('シミュレーション接続、キーマップ読み込み、登録画面をブラウザで通過する', async ({ page }) => {
    await page.goto('/');

    await page.getByRole('button', { name: 'シミュレーションに接続' }).click();
    await expect(page.getByText('シミュレーション 接続済み')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'ブリッジ管理' })).toBeVisible();

    await page.getByRole('button', { name: 'キーマップ', exact: true }).click();
    await expect(page.getByRole('heading', { name: '固定キーマップ' })).toBeVisible();
    await expect(page.getByText('保存先: シミュレーション')).toBeVisible();
    await page.getByRole('button', { name: 'キーマップを読み込む' }).click();
    await expect(page.getByText('0/32ルール・保存済み')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'ビジュアルキーマップ' })).toBeVisible();
    await page.getByRole('button', { name: 'A (0x04)', exact: true }).click();
    await page.getByRole('combobox', { name: '出力キー' }).selectOption('5');
    await expect(page.getByText('1/32ルール・未保存の変更')).toBeVisible();
    await page.getByRole('button', { name: 'キーマップを保存' }).click();
    await expect(page.getByText('1/32ルール・保存済み')).toBeVisible();

    await page.getByRole('button', { name: 'キーボード登録', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'キーボードの登録' })).toBeVisible();
    await expect(page.getByRole('heading', { name: '登録スロット' })).toBeVisible();
  });
});
