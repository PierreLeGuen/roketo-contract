import React from 'react';
import {TokenIcon} from '../../icons/Tokens';
import classNames from 'classnames';
export function TokenImage({tokenName, size = 8, className, ...rest}) {
  return (
    <div
      className={classNames(
        'p-1 flex-shrink-0 rounded-lg bg-card2 inline-flex items-center justify-center',
        `w-${size} h-${size}`,
        className,
      )}
      {...rest}
    >
      <TokenIcon className="w-full h-full rounded-full" tokenName={tokenName} />
    </div>
  );
}
