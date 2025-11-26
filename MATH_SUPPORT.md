# LaTeX Math Support in Plugable Chat

Plugable Chat now supports LaTeX math expressions using KaTeX, including the `\boxed{}` command.

## Supported Math Syntax

### Inline Math
Use single dollar signs for inline math:
- `$x^2 + y^2 = z^2$` renders as inline math
- `$\boxed{42}$` renders a boxed number inline
- `$\boxed{\text{Harrisburg}}$` renders boxed text inline

### Display Math
Use double dollar signs for display (centered) math:
```
$$
\boxed{x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}}
$$
```

### Boxed Expressions
The `\boxed{}` command creates a bordered box around the content:
- `$\boxed{\text{Harrisburg}}$` - boxes text (use `\text{}` for regular text)
- `$\boxed{x = 5}$` - boxes equations
- `$$\boxed{\text{Final Answer: } 42}$$` - display mode with box

## Examples

**Simple boxed answer:**
```
The capital of Pennsylvania is $\boxed{\text{Harrisburg}}$.
```

**Boxed equation:**
```
$$
\boxed{E = mc^2}
$$
```

**Complex expression:**
```
$$
\boxed{
  \int_{0}^{\infty} e^{-x^2} dx = \frac{\sqrt{\pi}}{2}
}
$$
```

**Multiple boxed items:**
```
The answer is $\boxed{x = 3}$ or $\boxed{x = -3}$.
```

## Styling

The boxed content is styled with:
- 2px solid border in dark grey (#2e2e2e)
- Light grey background (#f9f9f9)
- Rounded corners (4px border-radius)
- Appropriate padding for readability

All math expressions are styled to work well with the light theme interface.

## Troubleshooting

If `\boxed{}` doesn't render correctly:
1. Make sure you're using `$...$` for inline or `$$...$$` for display math
2. Use `\text{}` for regular text inside math mode: `$\boxed{\text{Answer}}$`
3. Check that the dollar signs are properly matched
4. The box is rendered using SVG by KaTeX and should appear automatically
