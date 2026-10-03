import { PageHeader } from "@/components/page-header";

/**
 * The page under first-run setup's dialog (`#/setup`).
 *
 * `SetupController` draws `SetupDialog` over whatever the content pane holds,
 * and for this address that used to be the Overview knowledge graph. The
 * Overview page was removed, so the pane is empty behind the dialog — but the
 * address still needs a name a screen reader can announce (issue #1763), and a
 * dialog is named by its own title rather than by the page. This is that name,
 * `hidden` because the dialog is the whole of what is on screen.
 */
export function SetupRouteView() {
  return (
    <div className="flex flex-1">
      <PageHeader title="Set up your company" hidden />
    </div>
  );
}
