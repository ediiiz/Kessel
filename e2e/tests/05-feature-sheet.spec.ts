import { expect, test } from '../support/fixtures.ts';

const TIME_LIGHT = 'Dishcare.Dishwasher.Setting.TimeLight';

test('any feature can be found by name and changed from the feature sheet', async ({
  page,
  device,
}) => {
  await page.goto(`/#/devices/${device}`);
  await page.getByTestId('open-features').click();

  const sheet = page.getByTestId('feature-sheet');
  await sheet.getByTestId('feature-search').fill('time beam');
  const row = sheet.locator(`[data-key="${TIME_LIGHT}"]`);
  await expect(sheet.getByTestId('feature-row')).toHaveCount(1);
  // Earlier journeys share this simulator, so flip to whichever value it is not holding now.
  const current = await row.getByTestId('feature-value').innerText();
  const next = current === 'On' ? 'Off' : 'On';

  await row.getByTestId('feature-toggle').click();
  await row.locator(`[data-testid="feature-option"][data-value="${next}"]`).click();

  // The catalogue path always asks first, and names what it is about to send.
  const confirm = page.getByTestId('write-confirm-panel');
  await expect(confirm.getByTestId('write-pending')).toContainText(`${TIME_LIGHT} = ${next}`);
  await confirm.getByTestId('write-confirm').click();

  await expect(page.getByText('Applied.')).toBeVisible();
  await expect(row.getByTestId('feature-value')).toHaveText(next);
});
