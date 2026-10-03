"use client"

import * as React from "react"
import { mergeProps } from "@base-ui/react/merge-props"
import { useRender } from "@base-ui/react/use-render"
import { cva, type VariantProps } from "class-variance-authority"

import { useIsMobile } from "@/hooks/use-mobile"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Separator } from "@/components/ui/separator"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { PanelLeftIcon } from "lucide-react"

const SIDEBAR_COOKIE_NAME = "sidebar_state"
const SIDEBAR_COOKIE_MAX_AGE = 60 * 60 * 24 * 7
// Wider than the shadcn default (16rem): the conversation rail docked here is
// two-line rows — a 40px face, the name with its timestamp, and a preview line
// under it — and at 15rem the name truncated after ten characters. Matches the
// mobile sheet's width so both densities read the same.
const SIDEBAR_WIDTH = "18rem"
const SIDEBAR_WIDTH_MOBILE = "18rem"
const SIDEBAR_WIDTH_ICON = "3rem"
const SIDEBAR_KEYBOARD_SHORTCUT = "b"

/**
 * The persisted open/closed state, or `undefined` if nothing has been stored.
 *
 * `setOpen` below has always written this cookie. Upstream shadcn reads it on
 * the server and hands it back as `defaultOpen`; this console has no server
 * render, so nothing ever read it and the write was dead — collapsing the
 * sidebar survived until the next reload and no further.
 *
 * That turned load-bearing when the channel list became a section of this
 * sidebar: the rail used to persist its own collapsed flag in `localStorage`
 * (`lib/chat-rail.ts`), and the sidebar's cookie was meant to replace it. A
 * write-only cookie replaces nothing, so the operator's density choice silently
 * became session-only (codex P2 review on #1987).
 */
function readSidebarCookie(): boolean | undefined {
  if (typeof document === "undefined") return undefined
  const match = document.cookie.match(
    new RegExp(`(?:^|;\\s*)${SIDEBAR_COOKIE_NAME}=(true|false)(?:;|$)`),
  )
  return match ? match[1] === "true" : undefined
}

type SidebarContextProps = {
  state: "expanded" | "collapsed"
  open: boolean
  setOpen: (open: boolean) => void
  openMobile: boolean
  setOpenMobile: (open: boolean) => void
  isMobile: boolean
  toggleSidebar: () => void
}

const SidebarContext = React.createContext<SidebarContextProps | null>(null)

function useSidebar() {
  const context = React.useContext(SidebarContext)
  if (!context) {
    throw new Error("useSidebar must be used within a SidebarProvider.")
  }

  return context
}

function SidebarProvider({
  defaultOpen = true,
  open: openProp,
  onOpenChange: setOpenProp,
  className,
  style,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  defaultOpen?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
}) {
  const isMobile = useIsMobile()
  const [openMobile, setOpenMobile] = React.useState(false)

  // This is the internal state of the sidebar.
  // We use openProp and setOpenProp for control from outside the component.
  // Seeded from the cookie `setOpen` writes, so the choice survives a reload.
  // See `readSidebarCookie`. `defaultOpen` still decides on a first visit.
  const [_open, _setOpen] = React.useState(() => readSidebarCookie() ?? defaultOpen)
  const open = openProp ?? _open
  const setOpen = React.useCallback(
    (value: boolean | ((value: boolean) => boolean)) => {
      const openState = typeof value === "function" ? value(open) : value
      if (setOpenProp) {
        setOpenProp(openState)
      } else {
        _setOpen(openState)
      }

      // This sets the cookie to keep the sidebar state.
      document.cookie = `${SIDEBAR_COOKIE_NAME}=${openState}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE}`
    },
    [setOpenProp, open]
  )

  // Helper to toggle the sidebar.
  const toggleSidebar = React.useCallback(() => {
    return isMobile ? setOpenMobile((open) => !open) : setOpen((open) => !open)
  }, [isMobile, setOpen, setOpenMobile])

  // Adds a keyboard shortcut to toggle the sidebar.
  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        event.key === SIDEBAR_KEYBOARD_SHORTCUT &&
        (event.metaKey || event.ctrlKey)
      ) {
        event.preventDefault()
        toggleSidebar()
      }
    }

    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [toggleSidebar])

  // We add a state so that we can do data-state="expanded" or "collapsed".
  // This makes it easier to style the sidebar with Tailwind classes.
  const state = open ? "expanded" : "collapsed"

  const contextValue = React.useMemo<SidebarContextProps>(
    () => ({
      state,
      open,
      setOpen,
      isMobile,
      openMobile,
      setOpenMobile,
      toggleSidebar,
    }),
    [state, open, setOpen, isMobile, openMobile, setOpenMobile, toggleSidebar]
  )

  return (
    <SidebarContext.Provider value={contextValue}>
      <div
        data-slot="sidebar-wrapper"
        style={
          {
            "--sidebar-width": SIDEBAR_WIDTH,
            "--sidebar-width-icon": SIDEBAR_WIDTH_ICON,
            ...style,
          } as React.CSSProperties
        }
        className={cn(
          // `bg-chrome` (issue #1178): this element is the chrome layer, and
          // the ONE place it is painted. Both the sidebar column
          // (`sidebar-inner`) and the content column (`SidebarInset`) are
          // transparent, so they are this fill showing through — which is what
          // makes them read as one continuous surface rather than two tinted
          // panes meeting at a seam. It lives here rather than on the caller so
          // the primitive is self-sufficient: a second provider anywhere would
          // otherwise render an unpainted shell with no error.
          "group/sidebar-wrapper flex h-svh min-h-svh w-full bg-chrome has-data-[variant=inset]:bg-sidebar",
          className
        )}
        {...props}
      >
        {children}
      </div>
    </SidebarContext.Provider>
  )
}

function Sidebar({
  side = "left",
  variant = "sidebar",
  collapsible = "offcanvas",
  className,
  children,
  dir,
  ...props
}: React.ComponentProps<"div"> & {
  side?: "left" | "right"
  variant?: "sidebar" | "floating" | "inset"
  collapsible?: "offcanvas" | "icon" | "none"
}) {
  const { isMobile, state, openMobile, setOpenMobile } = useSidebar()

  if (collapsible === "none") {
    return (
      <div
        data-slot="sidebar"
        className={cn(
          "flex h-full w-(--sidebar-width) flex-col bg-sidebar text-sidebar-foreground",
          className
        )}
        {...props}
      >
        {children}
      </div>
    )
  }

  if (isMobile) {
    return (
      <Sheet open={openMobile} onOpenChange={setOpenMobile} {...props}>
        <SheetContent
          dir={dir}
          data-sidebar="sidebar"
          data-slot="sidebar"
          data-mobile="true"
          aria-modal="true"
          onKeyDownCapture={(event) => {
            if (event.key === "Escape") {
              setOpenMobile(false)
            }
          }}
          className="w-(--sidebar-width) bg-sidebar p-0 text-sidebar-foreground [&>button]:hidden"
          style={
            {
              "--sidebar-width": SIDEBAR_WIDTH_MOBILE,
            } as React.CSSProperties
          }
          side={side}
        >
          <SheetHeader className="sr-only">
            <SheetTitle>Sidebar</SheetTitle>
            <SheetDescription>Displays the mobile sidebar.</SheetDescription>
          </SheetHeader>
          {/* The sheet's own scroller.
              `sidebar-inner` below is the desktop branch only, so when the
              column's single scroller moved there the sheet was left with none:
              a phone showing five nav rows, a channel list and every agent
              simply ran off the bottom with no way to reach the end. Same two
              classes, same reason — one scroller for the whole column — and the
              hover treatment is deliberately NOT applied, because a touch
              pointer never hovers and a bar it cannot summon is a bar it does
              not have. */}
          <div className="flex h-full w-full flex-col overflow-y-auto">{children}</div>
        </SheetContent>
      </Sheet>
    )
  }

  return (
    <div
      className="group peer hidden text-sidebar-foreground md:block"
      data-state={state}
      data-collapsible={state === "collapsed" ? collapsible : ""}
      data-variant={variant}
      data-side={side}
      data-slot="sidebar"
    >
      {/* This is what handles the sidebar gap on desktop */}
      <div
        data-slot="sidebar-gap"
        className={cn(
          "relative w-(--sidebar-width) bg-transparent transition-[width] duration-200 ease-linear",
          "group-data-[collapsible=offcanvas]:w-0",
          "group-data-[side=right]:rotate-180",
          variant === "floating" || variant === "inset"
            ? "group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+(--spacing(4)))]"
            : "group-data-[collapsible=icon]:w-(--sidebar-width-icon)"
        )}
      />
      <div
        data-slot="sidebar-container"
        data-side={side}
        className={cn(
          // `left: 0`, and nothing stands to the left of it any more.
          //
          // This container is `fixed`, so it positions against the VIEWPORT
          // rather than the column it is written inside. It used to be offset
          // by `--oc-rail-inset` because the connection rail occupied the first
          // 56px of the window and pinning this to 0 slid the whole sidebar
          // underneath it — every icon and the first characters of every label
          // clipped. Issue #1142 replaced that rail with a switcher in this
          // sidebar's own header, so the offset has nothing left to clear.
          "fixed inset-y-0 z-10 hidden h-svh w-(--sidebar-width) transition-[left,right,width] duration-200 ease-linear data-[side=left]:left-0 data-[side=left]:group-data-[collapsible=offcanvas]:left-[calc(var(--sidebar-width)*-1)] data-[side=right]:right-0 data-[side=right]:group-data-[collapsible=offcanvas]:right-[calc(var(--sidebar-width)*-1)] md:flex",
          // Adjust the padding for floating and inset variants.
          variant === "floating" || variant === "inset"
            ? "p-2 group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+(--spacing(4))+2px)]"
            // No `border-r` (issue #1178): the column and the content card
            // separate by fill contrast now, and a filled seam would draw a
            // line straight across the chrome that separation replaces.
            : "group-data-[collapsible=icon]:w-(--sidebar-width-icon)",
          className
        )}
        {...props}
      >
        <div
          data-sidebar="sidebar"
          data-slot="sidebar-inner"
          // No fill (issue #1178). The column sits directly on the window
          // chrome the shell root paints, and so does the margin around the
          // content card — one continuous surface, which is the whole point of
          // the two-layer shell. Painting `bg-sidebar` here would tint this
          // column separately from the frame beside it and re-draw the seam.
          //
          // The mobile sheet above keeps its fill: it is an overlay dragged
          // over the page, not a pane of the shell.
          // `overflow-y-auto` HERE, not on an inner region: the whole column
          // scrolls as one, the way Slack's does. It used to be
          // `SidebarContent` that scrolled, with the destinations pinned above
          // it and the channel list scrolling inside its own box — so the
          // column had two scroll positions and a channel list whose viewport
          // was whatever was left after the fixed rows, which on a short window
          // was a few rows tall while the column beside it sat still.
          //
          // One scroller means the nav rows scroll away with everything else,
          // which is the trade and is the right one: they are five rows at the
          // top of a list you are already scrolling, not chrome. What is
          // genuinely chrome — the switcher, Overview, Settings, Feedback,
          // Discord, the autonomy tier and you — is in the window's title row
          // and never moves.
          //
          // The bar appears on hover and is hidden otherwise
          // (`scrollbar-on-hover` in `index.css`). This column is standing
          // furniture behind every view, and a permanent bar down its edge is a
          // vertical rule the layout spent some effort removing — but it is
          // also the whole column that scrolls now, so an operator reaching for
          // it has to find it. Hover is where the reach begins, which makes it
          // the moment to show the bar and no earlier.
          className="scrollbar-on-hover flex size-full flex-col overflow-y-auto bg-transparent group-data-[variant=floating]:rounded-lg group-data-[variant=floating]:shadow-sm group-data-[variant=floating]:ring-1 group-data-[variant=floating]:ring-sidebar-border"
        >
          {children}
        </div>
      </div>
    </div>
  )
}

function SidebarTrigger({
  className,
  onClick,
  ...props
}: React.ComponentProps<typeof Button>) {
  const { toggleSidebar } = useSidebar()

  return (
    <Button
      data-sidebar="trigger"
      data-slot="sidebar-trigger"
      variant="ghost"
      size="icon-sm"
      className={cn(className)}
      onClick={(event) => {
        onClick?.(event)
        toggleSidebar()
      }}
      {...props}
    >
      <PanelLeftIcon />
      <span className="sr-only">Toggle Sidebar</span>
    </Button>
  )
}

function SidebarRail({ className, ...props }: React.ComponentProps<"button">) {
  const { toggleSidebar } = useSidebar()

  return (
    <button
      data-sidebar="rail"
      data-slot="sidebar-rail"
      aria-label="Toggle Sidebar"
      tabIndex={-1}
      onClick={toggleSidebar}
      title="Toggle Sidebar"
      className={cn(
        "absolute inset-y-0 z-20 hidden w-4 transition-all ease-linear group-data-[side=left]:-right-4 group-data-[side=right]:left-0 after:absolute after:inset-y-0 after:start-1/2 after:w-[2px] hover:after:bg-sidebar-border sm:flex ltr:-translate-x-1/2 rtl:-translate-x-1/2",
        "in-data-[side=left]:cursor-w-resize in-data-[side=right]:cursor-e-resize",
        "[[data-side=left][data-state=collapsed]_&]:cursor-e-resize [[data-side=right][data-state=collapsed]_&]:cursor-w-resize",
        "group-data-[collapsible=offcanvas]:translate-x-0 group-data-[collapsible=offcanvas]:after:left-full hover:group-data-[collapsible=offcanvas]:bg-sidebar",
        "[[data-side=left][data-collapsible=offcanvas]_&]:-right-2",
        "[[data-side=right][data-collapsible=offcanvas]_&]:-left-2",
        className
      )}
      {...props}
    />
  )
}

function SidebarInset({ className, ...props }: React.ComponentProps<"main">) {
  return (
    <main
      data-slot="sidebar-inset"
      className={cn(
        // `min-w-0` rather than `w-full`: as a flex child beside a fixed-width
        // sidebar, a 100% basis makes this box overflow the row by exactly the
        // sidebar's width. Nothing showed it while every view centred its own
        // max-width container, but a full-bleed view runs off the right edge.
        // `bg-transparent`, not `bg-background` (issue #1178). This is the
        // chrome-side column: the opaque sheet is the `ContentSurface` card
        // inside it, which carries `--background` and the inset margin. A fill
        // here would paint over the frame the card is supposed to float in.
        "relative flex min-w-0 flex-1 flex-col bg-transparent md:peer-data-[variant=inset]:m-2 md:peer-data-[variant=inset]:ml-0 md:peer-data-[variant=inset]:rounded-xl md:peer-data-[variant=inset]:shadow-sm md:peer-data-[variant=inset]:peer-data-[state=collapsed]:ml-2",
        className
      )}
      {...props}
    />
  )
}

function SidebarInput({
  className,
  ...props
}: React.ComponentProps<typeof Input>) {
  return (
    <Input
      data-slot="sidebar-input"
      data-sidebar="input"
      className={cn("h-8 w-full bg-background shadow-none", className)}
      {...props}
    />
  )
}

function SidebarHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-header"
      data-sidebar="header"
      // OpenHuman's shell gutter (`px-3`, `pt-3 pb-2`), with `gap-1` between
      // the blocks inside it — see `app/src/components/layout/shell` in the
      // vendored checkout. The rail keeps `px-2`: it is 3rem wide, so a 12px
      // gutter would leave 24px of content box and clip every 32px control in
      // this column. OpenHuman never hits that case because its sidebar hides
      // outright rather than collapsing to icons.
      className={cn(
        "flex flex-col gap-1 px-3 pt-3 pb-2",
        "group-data-[collapsible=icon]:px-2 group-data-[collapsible=icon]:items-center",
        className,
      )}
      {...props}
    />
  )
}

function SidebarFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-footer"
      data-sidebar="footer"
      // `pt-1 pb-2` against the header's `pt-3 pb-2`: the top of the column
      // carries the window's own breathing room, the bottom does not need it.
      // Same gutter and rail override as the header above.
      className={cn(
        "flex flex-col gap-1 px-3 pt-1 pb-2",
        "group-data-[collapsible=icon]:px-2",
        className,
      )}
      {...props}
    />
  )
}

function SidebarSeparator({
  className,
  ...props
}: React.ComponentProps<typeof Separator>) {
  return (
    <Separator
      data-slot="sidebar-separator"
      data-sidebar="separator"
      className={cn("mx-2 w-auto bg-sidebar-border", className)}
      {...props}
    />
  )
}

function SidebarContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-content"
      data-sidebar="content"
      // This region does not scroll and does not clip: `sidebar-inner` is the
      // column's single scroller (issue #1931 review, and the Slack-style
      // whole-column scroll that replaced it).
      //
      // It used to own the overflow, and answered it with `overflow-hidden`
      // while the rail's chrome was short enough that the nav list always fit.
      // That stopped being true — the macOS traffic-light inset stacks above
      // the nav in collapsed mode, and at the supported minimum window height
      // that stack plus a full nav list exceeded the rail, clipping the last
      // rows out of reach with no way to get to them.
      //
      // Moving the scroller to the whole column fixes the collapsed rail by
      // construction rather than by a second `collapsible=icon` rule: there is
      // one scrolling box, at every state. `flex-none` is what keeps this
      // region from absorbing the slack instead of the channel list below it.
      // `pt-4.5` (18px) starts the nav list level with the first row of the
      // page beside it, instead of at the top of its own column. Both columns
      // begin at the same y under the title row, but the content surface
      // spends its first band on a header — `ChatHeader`'s `h-15.5`, 62px with
      // its title vertically centred — while the sidebar put its first 32px row
      // flush against the top. The two first rows therefore sat 18px apart, and
      // the eye reads that as the column having slipped rather than as two
      // different kinds of thing.
      //
      // The number is that gap, derived rather than dialled: centring a 32px
      // row in the 62px header band is `(62 - 32) / 2` = 15px from the surface's
      // top edge, which sits 3px below the sidebar's. Views with the shorter
      // `h-13` header (52px) land 5px high of their own first row — worth it
      // for the view every session opens on, and the fix for those is to settle
      // on one header height rather than to average this.
      className={cn(
        "flex w-full flex-none flex-col gap-1 pt-4.5",
        className
      )}
      {...props}
    />
  )
}

function SidebarGroup({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-group"
      data-sidebar="group"
      // `pl-3` matches the header's gutter, so a nav row's icon and the
      // switcher's glyph stand on the same vertical line.
      //
      // The trailing gutter is the SCROLLBAR's width, not the leading one's.
      // With `pr-3` the rows stopped 12px short of the column edge and the
      // 10px scrollbar then sat in that gap with 2px to spare, so a row's right
      // edge lined up with nothing: not the bar beside it, and not the content
      // card past it. `--scrollbar-size` puts the row's edge exactly where the
      // bar begins, which makes the column read as one measure ending at the
      // card rather than as rows floating inside a wider box.
      //
      // No padding on TOP. `SidebarContent` already separates its children with
      // `gap-1`, so a `pt-1` here stacked on that gap and pushed every group
      // away from whatever sits above it — most visibly the first group under
      // the nav buttons, which read as belonging to a different column than the
      // rows they follow. Spacing between groups is the parent's job; this
      // keeps only the trailing breath that stops the last row of one group
      // touching the next group's label.
      className={cn(
        "relative flex w-full min-w-0 flex-col pb-1 pl-3 pr-(--scrollbar-size) group-data-[collapsible=icon]:px-2",
        className,
      )}
      {...props}
    />
  )
}

function SidebarGroupLabel({
  className,
  render,
  ...props
}: useRender.ComponentProps<"div"> & React.ComponentProps<"div">) {
  return useRender({
    defaultTagName: "div",
    props: mergeProps<"div">(
      {
        className: cn(
          "flex h-8 shrink-0 items-center rounded-md px-2 text-xs font-medium text-sidebar-foreground/70 ring-sidebar-ring outline-hidden transition-[margin,opacity] duration-200 ease-linear group-data-[collapsible=icon]:-mt-8 group-data-[collapsible=icon]:opacity-0 focus-visible:ring-2 [&>svg]:size-4 [&>svg]:shrink-0",
          className
        ),
      },
      props
    ),
    render,
    state: {
      slot: "sidebar-group-label",
      sidebar: "group-label",
    },
  })
}

function SidebarGroupAction({
  className,
  render,
  ...props
}: useRender.ComponentProps<"button"> & React.ComponentProps<"button">) {
  return useRender({
    defaultTagName: "button",
    props: mergeProps<"button">(
      {
        className: cn(
          "absolute top-3.5 right-3 flex aspect-square w-5 items-center justify-center rounded-md p-0 text-sidebar-foreground ring-sidebar-ring outline-hidden transition-transform group-data-[collapsible=icon]:hidden after:absolute after:-inset-2 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 md:after:hidden [&>svg]:size-4 [&>svg]:shrink-0",
          className
        ),
      },
      props
    ),
    render,
    state: {
      slot: "sidebar-group-action",
      sidebar: "group-action",
    },
  })
}

function SidebarGroupContent({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-group-content"
      data-sidebar="group-content"
      className={cn("w-full text-sm", className)}
      {...props}
    />
  )
}

function SidebarMenu({ className, ...props }: React.ComponentProps<"ul">) {
  return (
    <ul
      data-slot="sidebar-menu"
      data-sidebar="menu"
      className={cn("flex w-full min-w-0 flex-col gap-0.5", className)}
      {...props}
    />
  )
}

function SidebarMenuItem({ className, ...props }: React.ComponentProps<"li">) {
  return (
    <li
      data-slot="sidebar-menu-item"
      data-sidebar="menu-item"
      className={cn("group/menu-item relative", className)}
      {...props}
    />
  )
}

const sidebarMenuButtonVariants = cva(
  "peer/menu-button group/menu-button flex w-full items-center gap-2 overflow-hidden rounded-md p-2 text-left text-sm ring-sidebar-ring outline-hidden transition-[width,height,padding] group-has-data-[sidebar=menu-action]/menu-item:pr-8 group-data-[collapsible=icon]:size-8! group-data-[collapsible=icon]:p-2! hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 data-open:hover:bg-sidebar-accent data-open:hover:text-sidebar-accent-foreground data-active:bg-sidebar-accent data-active:font-medium data-active:text-sidebar-accent-foreground [&_svg]:size-4 [&_svg]:shrink-0 [&>span:last-child]:truncate",
  {
    variants: {
      variant: {
        default: "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        outline:
          "bg-background shadow-[0_0_0_1px_var(--sidebar-border)] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:shadow-[0_0_0_1px_var(--sidebar-accent)]",
      },
      size: {
        default: "h-8 text-sm",
        sm: "h-7 text-xs",
        lg: "h-12 text-sm group-data-[collapsible=icon]:p-0!",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function SidebarMenuButton({
  render,
  isActive = false,
  variant = "default",
  size = "default",
  tooltip,
  className,
  ...props
}: useRender.ComponentProps<"button"> &
  React.ComponentProps<"button"> & {
    isActive?: boolean
    tooltip?: string | React.ComponentProps<typeof TooltipContent>
  } & VariantProps<typeof sidebarMenuButtonVariants>) {
  const { isMobile, state } = useSidebar()
  const comp = useRender({
    defaultTagName: "button",
    props: mergeProps<"button">(
      {
        className: cn(sidebarMenuButtonVariants({ variant, size }), className),
      },
      props
    ),
    render: !tooltip ? render : <TooltipTrigger render={render} />,
    state: {
      slot: "sidebar-menu-button",
      sidebar: "menu-button",
      size,
      active: isActive,
    },
  })

  if (!tooltip) {
    return comp
  }

  if (typeof tooltip === "string") {
    tooltip = {
      children: tooltip,
    }
  }

  return (
    <Tooltip>
      {comp}
      <TooltipContent
        side="right"
        align="center"
        hidden={state !== "collapsed" || isMobile}
        {...tooltip}
      />
    </Tooltip>
  )
}

function SidebarMenuAction({
  className,
  render,
  showOnHover = false,
  ...props
}: useRender.ComponentProps<"button"> &
  React.ComponentProps<"button"> & {
    showOnHover?: boolean
  }) {
  return useRender({
    defaultTagName: "button",
    props: mergeProps<"button">(
      {
        className: cn(
          "absolute top-1.5 right-1 flex aspect-square w-5 items-center justify-center rounded-md p-0 text-sidebar-foreground ring-sidebar-ring outline-hidden transition-transform group-data-[collapsible=icon]:hidden peer-hover/menu-button:text-sidebar-accent-foreground peer-data-[size=default]/menu-button:top-1.5 peer-data-[size=lg]/menu-button:top-2.5 peer-data-[size=sm]/menu-button:top-1 after:absolute after:-inset-2 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 md:after:hidden [&>svg]:size-4 [&>svg]:shrink-0",
          showOnHover &&
            "group-focus-within/menu-item:opacity-100 group-hover/menu-item:opacity-100 peer-data-active/menu-button:text-sidebar-accent-foreground aria-expanded:opacity-100 md:opacity-0",
          className
        ),
      },
      props
    ),
    render,
    state: {
      slot: "sidebar-menu-action",
      sidebar: "menu-action",
    },
  })
}

function SidebarMenuBadge({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-menu-badge"
      data-sidebar="menu-badge"
      className={cn(
        "pointer-events-none absolute right-1 flex h-5 min-w-5 items-center justify-center rounded-md px-1 text-xs font-medium text-sidebar-foreground tabular-nums select-none group-data-[collapsible=icon]:hidden peer-hover/menu-button:text-sidebar-accent-foreground peer-data-[size=default]/menu-button:top-1.5 peer-data-[size=lg]/menu-button:top-2.5 peer-data-[size=sm]/menu-button:top-1 peer-data-active/menu-button:text-sidebar-accent-foreground",
        className
      )}
      {...props}
    />
  )
}

/**
 * The attention mark that survives the rail collapsing (issue #1018).
 *
 * `SidebarMenuBadge` above carries `group-data-[collapsible=icon]:hidden`, so a
 * count rendered only there vanishes the moment the sidebar collapses to icons
 * — and a collapsed rail showing nothing is indistinguishable from all-clear.
 * This is the same value rendered small enough to survive 32px, and the two are
 * mirrored so exactly one of them is ever on screen.
 *
 * It was deleted once, when Approvals was chrome in the window's title row: the
 * title row does not collapse, so the disappearance this guards against could
 * not happen, and a second mechanism for a count that had left the sidebar was
 * worth removing. Approvals is a sidebar row again, so the hazard is back and so
 * is this. Whichever of the two moves, they move together — a badge with no dot
 * is issue #1018 verbatim.
 */
function SidebarMenuDot({ className, ...props }: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="sidebar-menu-dot"
      data-sidebar="menu-dot"
      aria-hidden="true"
      className={cn(
        // The mirror of the badge's own hide rule: shown ONLY on the icon rail.
        "pointer-events-none absolute top-1.5 right-1.5 hidden size-2 rounded-full",
        "bg-status-blocked-text group-data-[collapsible=icon]:block",
        className
      )}
      {...props}
    />
  )
}

function SidebarMenuSkeleton({
  className,
  showIcon = false,
  ...props
}: React.ComponentProps<"div"> & {
  showIcon?: boolean
}) {
  // Random width between 50 to 90%.
  const [width] = React.useState(() => {
    return `${Math.floor(Math.random() * 40) + 50}%`
  })

  return (
    <div
      data-slot="sidebar-menu-skeleton"
      data-sidebar="menu-skeleton"
      className={cn("flex h-8 items-center gap-2 rounded-md px-2", className)}
      {...props}
    >
      {showIcon && (
        <Skeleton
          className="size-4 rounded-md"
          data-sidebar="menu-skeleton-icon"
        />
      )}
      <Skeleton
        className="h-4 max-w-(--skeleton-width) flex-1"
        data-sidebar="menu-skeleton-text"
        style={
          {
            "--skeleton-width": width,
          } as React.CSSProperties
        }
      />
    </div>
  )
}

function SidebarMenuSub({ className, ...props }: React.ComponentProps<"ul">) {
  return (
    <ul
      data-slot="sidebar-menu-sub"
      data-sidebar="menu-sub"
      className={cn(
        "mx-3.5 flex min-w-0 translate-x-px flex-col gap-1 border-l border-sidebar-border px-2.5 py-0.5 group-data-[collapsible=icon]:hidden",
        className
      )}
      {...props}
    />
  )
}

function SidebarMenuSubItem({
  className,
  ...props
}: React.ComponentProps<"li">) {
  return (
    <li
      data-slot="sidebar-menu-sub-item"
      data-sidebar="menu-sub-item"
      className={cn("group/menu-sub-item relative", className)}
      {...props}
    />
  )
}

function SidebarMenuSubButton({
  render,
  size = "md",
  isActive = false,
  className,
  ...props
}: useRender.ComponentProps<"a"> &
  React.ComponentProps<"a"> & {
    size?: "sm" | "md"
    isActive?: boolean
  }) {
  return useRender({
    defaultTagName: "a",
    props: mergeProps<"a">(
      {
        className: cn(
          "flex h-7 min-w-0 -translate-x-px items-center gap-2 overflow-hidden rounded-md px-2 text-sidebar-foreground ring-sidebar-ring outline-hidden group-data-[collapsible=icon]:hidden hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 data-[size=md]:text-sm data-[size=sm]:text-xs data-active:bg-sidebar-accent data-active:text-sidebar-accent-foreground [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-sidebar-accent-foreground",
          className
        ),
      },
      props
    ),
    render,
    state: {
      slot: "sidebar-menu-sub-button",
      sidebar: "menu-sub-button",
      size,
      active: isActive,
    },
  })
}

export {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInput,
  SidebarInset,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuBadge,
  SidebarMenuDot,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSkeleton,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarProvider,
  SidebarRail,
  SidebarSeparator,
  SidebarTrigger,
  useSidebar,
}
