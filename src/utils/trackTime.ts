export function formatTrackTime(value: number | null | undefined): string {
  if (!value) return '';
  const min = ~~(value / 60);
  const sec = (~~(value % 60)).toString().padStart(2, '0');
  return `${min}:${sec}`;
}
