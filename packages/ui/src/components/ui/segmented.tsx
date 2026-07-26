import { cn } from "../../lib/utils";

type SegmentedOption<T extends string> = { id: T; label: string };

type SegmentedProps<T extends string> = {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (id: T) => void;
  size?: "sm" | "md";
  pill?: boolean;
  disabled?: boolean;
};

// Segmented control: muted track, bordered raised segment for the selection.
function Segmented<T extends string>({ options, value, onChange, size = "md", pill = false, disabled = false }: SegmentedProps<T>) {
  const h = size === "sm" ? "h-[23px] px-[9px] text-[11px]" : "h-[25px] px-2.5 text-[11.5px]";
  return (
    <div className={cn("flex w-fit gap-[2px] rounded-md bg-muted p-[2px]", disabled && "opacity-50")}>
      {options.map((o) => {
        const sel = o.id === value;
        return (
          <button
            key={o.id}
            type="button"
            aria-pressed={sel}
            disabled={disabled}
            onClick={() => onChange(o.id)}
            className={cn(
              h,
              "cursor-pointer font-medium disabled:cursor-not-allowed",
              pill ? "rounded-full" : "rounded-sm",
              "border",
              sel ? "border-border bg-background text-foreground" : "border-transparent bg-transparent text-muted-foreground",
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export { Segmented };
export type { SegmentedOption };
