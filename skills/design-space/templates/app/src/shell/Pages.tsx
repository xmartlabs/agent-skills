// Shell pages shared by every section.

export function NotFound({ what = "page" }: { what?: string }) {
  return (
    <div className="ds-page">
      <header className="ds-page-head">
        <h1 className="ds-page-title">Not found</h1>
        <p className="ds-lede">There is no {what} here. Pick one from the index, or go <a className="ds-link" href="#/">home</a>.</p>
      </header>
    </div>
  );
}
